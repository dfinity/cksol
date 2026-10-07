use crate::{
    constants::MAX_CONCURRENT_RPC_CALLS,
    deposit::sweep::credit_finalized_sweeps,
    guard::TimerGuard,
    monitor::withdrawals::check_withdrawal_transactions,
    rpc::{BlockHeight, get_recent_block, get_signature_statuses},
    runtime::CanisterRuntime,
    state::{
        MinterTransaction, TaskType, audit::process_event, event::EventType, mutate_state,
        read_state,
    },
};
use canlog::log;
use cksol_types_internal::log::Priority;
use itertools::Itertools;
use solana_signature::Signature;
use solana_transaction_status_client_types::TransactionConfirmationStatus;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

#[cfg(test)]
mod tests;
mod withdrawals;

pub const FINALIZE_TRANSACTIONS_DELAY: Duration = Duration::from_mins(2);
/// Minimum time since its submission before a withdrawal transaction whose nonce
/// account still stores its nonce value is re-broadcast. The minter sends
/// transactions without `maxRetries`, so an Agave RPC node keeps re-sending a
/// durable-nonce transaction every 2 s until it lands, its nonce advances, or
/// 150 blocks (about 60 to 90 s) pass.
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

/// Check the status of all submitted sweeps, finalize succeeded/failed ones and drop
/// expired ones, then finalize the withdrawals whose nonce advanced and re-broadcast
/// the withdrawals whose nonce did not.
pub async fn finalize_transactions<R: CanisterRuntime>(runtime: R) {
    let _guard = match TimerGuard::new(TaskType::FinalizeTransactions) {
        Ok(guard) => guard,
        Err(_) => return,
    };

    let reschedule = scopeguard::guard(runtime.clone(), |runtime| {
        runtime.set_timer(Duration::ZERO, finalize_transactions);
    });

    let more_transactions_to_check = check_sweep_transactions(&runtime).await;
    check_withdrawal_transactions(&runtime).await;
    let more_sweeps_to_credit = credit_finalized_sweeps(&runtime).await;

    if !more_transactions_to_check && !more_sweeps_to_credit {
        scopeguard::ScopeGuard::into_inner(reschedule);
    }
}

/// Returns whether the finalization timer must run again immediately.
async fn check_sweep_transactions<R: CanisterRuntime>(runtime: &R) -> bool {
    let blockhash_transactions: BTreeMap<Signature, BlockHeight> = read_state(|state| {
        state
            .submitted_transactions()
            .iter()
            .filter_map(|(signature, transaction)| match transaction {
                MinterTransaction::SweepDeposit { block_height } => {
                    Some((*signature, *block_height))
                }
                MinterTransaction::Withdrawal { .. } => None,
            })
            .collect()
    });
    if blockhash_transactions.is_empty() {
        return false;
    }

    // Fetch the current block before checking statuses: if a transaction finalizes
    // after we snapshot the block, the status check will see it as finalized rather
    // than missing, so it will never be incorrectly marked as expired.
    let current_block_height = fetch_current_block_height(runtime).await;

    let num_transactions = blockhash_transactions.len();
    let statuses =
        check_transaction_statuses(runtime, blockhash_transactions.keys().copied().collect()).await;

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

    num_transactions > MAX_CONCURRENT_RPC_CALLS * MAX_SIGNATURES_PER_STATUS_CHECK
}

async fn fetch_current_block_height<R: CanisterRuntime>(runtime: &R) -> Option<BlockHeight> {
    match get_recent_block(runtime).await {
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
