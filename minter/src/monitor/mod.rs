use crate::{
    constants::MAX_CONCURRENT_RPC_CALLS,
    deposit::sweep::credit_finalized_sweeps,
    guard::TimerGuard,
    rpc::{
        BlockHeight, get_recent_block, get_signature_statuses,
        submit_transaction_skipping_preflight,
    },
    runtime::CanisterRuntime,
    state::{
        MinterTransaction, TaskType,
        audit::process_event,
        event::{EventType, VersionedMessage},
        mutate_state, read_state,
    },
    storage::with_unstable_metrics_mut,
};
use canlog::log;
use cksol_types_internal::log::Priority;
use itertools::Itertools;
use sol_rpc_types::CommitmentLevel;
use solana_signature::Signature;
use solana_transaction::Transaction;
use solana_transaction_status_client_types::TransactionConfirmationStatus;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

#[cfg(test)]
mod tests;

pub const FINALIZE_TRANSACTIONS_DELAY: Duration = Duration::from_mins(2);
/// Minimum time since its submission before a withdrawal transaction without a
/// status is re-broadcast. The minter sends transactions without `maxRetries`,
/// so an Agave RPC node keeps re-sending a durable-nonce transaction every 2 s
/// until it lands, its nonce advances, or 150 blocks (about 60 to 90 s) pass.
/// Re-broadcasting earlier would only duplicate the work of the RPC node.
/// See https://github.com/anza-xyz/agave/blob/master/rpc/src/rpc.rs
pub const MIN_REBROADCAST_AGE: Duration = Duration::from_secs(90);
/// A leader accepts a transaction while its blockhash is still among the last
/// `MAX_PROCESSING_AGE` entries of the recent-blockhash queue, which holds one
/// entry per non-skipped slot. The public documentation describes this window
/// as 150 slots, but the validator counts blocks: in the `solana-clock` crate,
/// `MAX_PROCESSING_AGE = MAX_RECENT_BLOCKHASHES / 2` and
/// `MAX_RECENT_BLOCKHASHES = MAX_HASH_AGE_IN_SECONDS * DEFAULT_TICKS_PER_SECOND
/// / DEFAULT_TICKS_PER_SLOT`, asserted to be 150 and 300 respectively.
/// See https://github.com/anza-xyz/agave/blob/master/sdk/clock/src/lib.rs
const MAX_BLOCKHASH_AGE_IN_BLOCKS: BlockHeight = BlockHeight::new(150);
/// Maximum number of signatures per `getSignatureStatuses` RPC call.
/// See https://solana.com/docs/rpc/http/getsignaturestatuses
const MAX_SIGNATURES_PER_STATUS_CHECK: usize = 256;

/// Check the status of all submitted transactions, finalize succeeded/failed
/// ones, drop expired sweeps, and re-broadcast withdrawals that have no status.
pub async fn finalize_transactions<R: CanisterRuntime>(runtime: R) {
    let _guard = match TimerGuard::new(TaskType::FinalizeTransactions) {
        Ok(guard) => guard,
        Err(_) => return,
    };

    let reschedule = scopeguard::guard(runtime.clone(), |runtime| {
        runtime.set_timer(Duration::ZERO, finalize_transactions);
    });

    let more_transactions_to_check = check_submitted_transactions(&runtime).await;
    let more_sweeps_to_credit = credit_finalized_sweeps(&runtime).await;

    if !more_transactions_to_check && !more_sweeps_to_credit {
        scopeguard::ScopeGuard::into_inner(reschedule);
    }
}

/// Returns whether the finalization timer must run again immediately.
async fn check_submitted_transactions<R: CanisterRuntime>(runtime: &R) -> bool {
    let (signatures, blockhash_transactions): (
        BTreeSet<Signature>,
        BTreeMap<Signature, BlockHeight>,
    ) = read_state(|state| {
        let submitted = state.submitted_transactions();
        (
            submitted.iter().map(|(sig, _)| *sig).collect(),
            submitted
                .iter()
                .filter_map(|(sig, tx)| match tx {
                    MinterTransaction::SweepDeposit { block_height, .. } => {
                        Some((*sig, *block_height))
                    }
                    MinterTransaction::Withdrawal { .. } => None,
                })
                .collect(),
        )
    });
    if signatures.is_empty() {
        return false;
    }

    // Fetch the current block before checking statuses: if a transaction finalizes
    // after we snapshot the block, the status check will see it as finalized rather
    // than missing, so it will never be incorrectly marked as expired.
    let current_block_height = if blockhash_transactions.is_empty() {
        None
    } else {
        fetch_current_block_height(runtime).await
    };

    let num_transactions = signatures.len();
    let statuses = check_transaction_statuses(runtime, signatures.into_iter().collect()).await;

    for (signature, error) in &statuses.errored {
        log!(
            Priority::Error,
            "Transaction {signature} finalized with on-chain error: {error}"
        );
        mutate_state(|state| {
            process_event(
                state,
                EventType::FailedTransaction {
                    signature: *signature,
                },
                runtime,
            )
        });
    }

    for signature in &statuses.succeeded {
        log!(Priority::Info, "Transaction {signature} finalized");
        mutate_state(|state| {
            process_event(
                state,
                EventType::SucceededTransaction {
                    signature: *signature,
                },
                runtime,
            )
        });
    }

    if let Some(current_block_height) = current_block_height {
        expire_transactions(
            runtime,
            &statuses.not_found,
            &blockhash_transactions,
            current_block_height,
        );
    }

    rebroadcast_withdrawal_transactions(runtime, &statuses.not_found).await;

    num_transactions > MAX_CONCURRENT_RPC_CALLS * MAX_SIGNATURES_PER_STATUS_CHECK
}

async fn fetch_current_block_height<R: CanisterRuntime>(runtime: &R) -> Option<BlockHeight> {
    match get_recent_block(runtime, CommitmentLevel::Finalized).await {
        Ok(block) => Some(block.block_height),
        Err(e) => {
            log!(
                Priority::Info,
                "Failed to get current block, skipping the expiry check this round: {e}"
            );
            None
        }
    }
}

fn expire_transactions<R: CanisterRuntime>(
    runtime: &R,
    not_found: &BTreeSet<Signature>,
    blockhash_transactions: &BTreeMap<Signature, BlockHeight>,
    current_block_height: BlockHeight,
) {
    for signature in not_found {
        let Some(transaction_block_height) = blockhash_transactions.get(signature) else {
            continue;
        };
        if !is_blockhash_expired(*transaction_block_height, current_block_height) {
            continue;
        }
        log!(Priority::Info, "Transaction {signature} expired");
        mutate_state(|state| {
            process_event(
                state,
                EventType::ExpiredTransaction {
                    signature: *signature,
                },
                runtime,
            )
        });
    }
}

async fn rebroadcast_withdrawal_transactions<R: CanisterRuntime>(
    runtime: &R,
    not_found: &BTreeSet<Signature>,
) {
    let now = runtime.time();
    let batches: Vec<Vec<Transaction>> = read_state(|state| {
        not_found
            .iter()
            .filter_map(
                |signature| match state.submitted_transactions().get(signature)? {
                    MinterTransaction::Withdrawal {
                        message: VersionedMessage::Legacy(message),
                        submitted_at,
                        ..
                    } if is_old_enough_to_rebroadcast(*submitted_at, now) => Some(Transaction {
                        signatures: vec![*signature],
                        message: message.clone(),
                    }),
                    MinterTransaction::Withdrawal { .. }
                    | MinterTransaction::SweepDeposit { .. } => None,
                },
            )
            .chunks(MAX_CONCURRENT_RPC_CALLS)
            .into_iter()
            .map(Iterator::collect)
            .collect()
    });
    for batch in batches {
        futures::future::join_all(
            batch
                .into_iter()
                .map(|transaction| rebroadcast_transaction(runtime, transaction)),
        )
        .await;
    }
}

fn is_old_enough_to_rebroadcast(submitted_at: u64, now: u64) -> bool {
    Duration::from_nanos(now.saturating_sub(submitted_at)) >= MIN_REBROADCAST_AGE
}

async fn rebroadcast_transaction<R: CanisterRuntime>(runtime: &R, transaction: Transaction) {
    let signature = transaction.signatures[0];
    with_unstable_metrics_mut(|m| m.withdrawal_transaction_rebroadcasts += 1);
    match submit_transaction_skipping_preflight(runtime, transaction).await {
        Ok(_) => log!(
            Priority::Info,
            "Re-broadcast withdrawal transaction {signature}"
        ),
        Err(e) => log!(
            Priority::Info,
            "Failed to re-broadcast withdrawal transaction {signature} (will retry next round): {e}"
        ),
    }
}

fn is_blockhash_expired(
    transaction_block_height: BlockHeight,
    current_block_height: BlockHeight,
) -> bool {
    current_block_height.saturating_sub(transaction_block_height) > MAX_BLOCKHASH_AGE_IN_BLOCKS
}

/// Result of checking transaction statuses.
// Transactions that are in-flight (Processed/Confirmed) or whose status
// check failed are implicitly excluded from the below sets.
struct TransactionStatuses {
    /// Transactions confirmed as finalized on-chain without errors.
    succeeded: BTreeSet<Signature>,
    /// Transactions that finalized with an on-chain error.
    errored: BTreeMap<Signature, String>,
    /// Transactions with no on-chain status.
    not_found: BTreeSet<Signature>,
}

async fn check_transaction_statuses<R: CanisterRuntime>(
    runtime: &R,
    signatures: Vec<Signature>,
) -> TransactionStatuses {
    let batches: Vec<Vec<_>> = signatures
        .into_iter()
        .chunks(MAX_SIGNATURES_PER_STATUS_CHECK)
        .into_iter()
        .take(MAX_CONCURRENT_RPC_CALLS)
        .map(Iterator::collect)
        .collect();

    let mut result = TransactionStatuses {
        succeeded: BTreeSet::new(),
        errored: BTreeMap::new(),
        not_found: BTreeSet::new(),
    };

    let batch_results: Vec<_> = futures::future::join_all(batches.into_iter().map(async |batch| {
        match get_signature_statuses(runtime, &batch).await {
            Ok(statuses) => Some((batch, statuses)),
            Err(e) => {
                log!(Priority::Info, "Failed to check transaction statuses: {e}");
                None
            }
        }
    }))
    .await;

    for (sigs, statuses) in batch_results.into_iter().flatten() {
        for (signature, status) in sigs.iter().zip(statuses) {
            match status {
                Some(s)
                    if s.confirmation_status == Some(TransactionConfirmationStatus::Finalized) =>
                {
                    if let Some(err) = s.err {
                        result.errored.insert(*signature, format!("{err:?}"));
                    } else {
                        result.succeeded.insert(*signature);
                    }
                }
                Some(_) => {} // in-flight (Processed/Confirmed)
                None => {
                    result.not_found.insert(*signature);
                }
            }
        }
    }

    result
}
