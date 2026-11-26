use crate::{
    constants::MAX_CONCURRENT_RPC_CALLS,
    deposit::{
        spl::{credit_finalized_spl_sweeps, settle_failed_spl_sweeps},
        sweep::credit_finalized_sweeps,
    },
    guard::TimerGuard,
    monitor::withdrawals::{
        finalize_transactions_batch, read_in_flight_nonces, resubmit_transactions_batch,
    },
    queue_cursor::QueueCursor,
    rpc::{BlockHeight, get_recent_block, get_signature_statuses},
    runtime::CanisterRuntime,
    state::{
        MinterTransaction, State, TaskType,
        audit::{process_dropped_sweep, process_event},
        event::EventType,
        mutate_state, read_state,
    },
};
use canlog::log;
use cksol_types_internal::log::Priority;
use itertools::Itertools;
use sol_rpc_types::CommitmentLevel;
use solana_signature::Signature;
use solana_transaction_status_client_types::TransactionConfirmationStatus;
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

#[cfg(test)]
mod tests;
mod withdrawals;

pub const FINALIZE_TRANSACTIONS_DELAY: Duration = Duration::from_mins(2);
pub const FINALIZE_TRANSACTIONS_RETRY_DELAY: Duration = Duration::from_secs(10);
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
/// Maximum number of sweep signatures whose status one round checks.
const MAX_SIGNATURES_PER_ROUND: usize = MAX_CONCURRENT_RPC_CALLS * MAX_SIGNATURES_PER_STATUS_CHECK;

thread_local! {
    // Rotates the status checks only when the in-flight sweeps exceed one round.
    static STATUS_CURSOR: RefCell<QueueCursor<Signature>> = const { RefCell::new(QueueCursor::new()) };
}

#[cfg(any(test, feature = "canbench-rs"))]
pub(crate) fn reset_cursor() {
    STATUS_CURSOR.with(|cursor| cursor.borrow_mut().clear());
}

/// Check the status of all submitted SOL and SPL sweeps, finalize succeeded/failed ones and
/// drop expired ones, then finalize the withdrawals whose nonce advanced and re-broadcast
/// the withdrawals whose nonce did not. Finalized sweeps settle in separate steps.
pub async fn finalize_transactions<R: CanisterRuntime>(runtime: R) {
    let _guard = match TimerGuard::new(TaskType::FinalizeTransactions) {
        Ok(guard) => guard,
        Err(_) => return,
    };

    let reschedule = scopeguard::guard(runtime.clone(), |runtime| {
        runtime.set_timer(FINALIZE_TRANSACTIONS_RETRY_DELAY, finalize_transactions);
    });

    let more_transactions_to_check = check_sweep_transactions(&runtime).await;
    let nonces = read_in_flight_nonces(&runtime).await;
    resubmit_transactions_batch(&runtime, &nonces).await;
    finalize_transactions_batch(&runtime, &nonces).await;
    let more_sweeps_to_credit = credit_finalized_sweeps(&runtime).await;
    let more_spl_sweeps_to_settle = settle_failed_spl_sweeps(&runtime).await;
    let more_spl_sweeps_to_credit = credit_finalized_spl_sweeps(&runtime).await;

    if !more_transactions_to_check
        && !more_sweeps_to_credit
        && !more_spl_sweeps_to_settle
        && !more_spl_sweeps_to_credit
    {
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
                MinterTransaction::SweepDeposit { block_height }
                | MinterTransaction::SweepSplDeposit { block_height } => {
                    Some((*signature, *block_height))
                }
                MinterTransaction::Withdrawal { .. } | MinterTransaction::SplWithdrawal { .. } => {
                    None
                }
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
    let statuses = check_transaction_statuses(runtime, status_round(&blockhash_transactions)).await;

    for (signature, error) in &statuses.errored {
        log!(
            Priority::Error,
            "Transaction {signature} finalized with on-chain error: {error}"
        );
        let event = EventType::FailedTransaction {
            signature: *signature,
        };
        mutate_state(|state| record_unsuccessful_sweep(state, signature, event, runtime));
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

    num_transactions > MAX_SIGNATURES_PER_ROUND
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
        let event = EventType::ExpiredTransaction {
            signature: *signature,
        };
        mutate_state(|state| record_unsuccessful_sweep(state, signature, event, runtime));
    }
}

/// Records a failed or expired sweep. Only a SOL sweep re-arms its automatic deposits.
fn record_unsuccessful_sweep<R: CanisterRuntime>(
    state: &mut State,
    signature: &Signature,
    event: EventType,
    runtime: &R,
) {
    if state.deposits().swept().get(signature).is_some() {
        process_dropped_sweep(state, event, runtime);
    } else {
        process_event(state, event, runtime);
    }
}

/// The signatures to check this round. All of them, in order, when one round covers them.
fn status_round(transactions: &BTreeMap<Signature, BlockHeight>) -> Vec<Signature> {
    STATUS_CURSOR.with(|cursor| {
        let mut cursor = cursor.borrow_mut();
        if transactions.len() <= MAX_SIGNATURES_PER_ROUND {
            cursor.clear();
            return transactions.keys().copied().collect();
        }
        let round: Vec<Signature> = cursor
            .round(transactions, MAX_SIGNATURES_PER_ROUND)
            .into_iter()
            .map(|(signature, _)| *signature)
            .collect();
        if let Some(last) = round.last() {
            cursor.advance(*last);
        }
        round
    })
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
            Ok(statuses) if statuses.len() == batch.len() => Some((batch, statuses)),
            Ok(statuses) => {
                // A malformed reply gives no status and no proof of expiry.
                log!(
                    Priority::Error,
                    "Ignoring {} statuses returned for {} signatures",
                    statuses.len(),
                    batch.len()
                );
                None
            }
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
