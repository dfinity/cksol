use crate::{
    address::derivation_path,
    constants::MAX_CONCURRENT_RPC_CALLS,
    deposit::sweep::credit_finalized_sweeps,
    guard::TimerGuard,
    rpc::{
        Block, BlockHeight, SubmitTransactionError, get_recent_block, get_signature_statuses,
        submit_transaction,
    },
    runtime::CanisterRuntime,
    signer::sign_bytes,
    state::{
        TaskType,
        audit::process_event,
        event::{EventType, VersionedMessage},
        mutate_state, read_state,
    },
};
use canlog::log;
use cksol_types_internal::log::Priority;
use ic_cdk_management_canister::SignCallError;
use icrc_ledger_types::icrc1::account::Account;
use itertools::Itertools;
use solana_signature::Signature;
use solana_transaction::Transaction;
use solana_transaction_status_client_types::TransactionConfirmationStatus;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;
use thiserror::Error;

#[cfg(test)]
mod tests;

pub const FINALIZE_TRANSACTIONS_DELAY: Duration = Duration::from_mins(2);
pub const RESUBMIT_TRANSACTIONS_DELAY: Duration = Duration::from_mins(3);
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
/// ones, and mark expired transactions for resubmission.
pub async fn finalize_transactions<R: CanisterRuntime>(runtime: R) {
    let _guard = match TimerGuard::new(TaskType::FinalizeTransactions) {
        Ok(guard) => guard,
        Err(_) => return,
    };

    check_submitted_transactions(&runtime).await;
    credit_finalized_sweeps(&runtime).await;
}

async fn check_submitted_transactions<R: CanisterRuntime>(runtime: &R) {
    let all_transactions: BTreeMap<Signature, BlockHeight> = read_state(|state| {
        state
            .submitted_transactions()
            .iter()
            .map(|(sig, tx)| (*sig, tx.block_height))
            .collect()
    });
    if all_transactions.is_empty() {
        return;
    }

    let more_to_process =
        all_transactions.len() > MAX_CONCURRENT_RPC_CALLS * MAX_SIGNATURES_PER_STATUS_CHECK;
    let reschedule = scopeguard::guard(runtime.clone(), |runtime| {
        runtime.set_timer(Duration::ZERO, finalize_transactions);
    });

    // Fetch the current block before checking statuses: if a transaction finalizes
    // after we snapshot the block, the status check will see it as finalized rather
    // than missing, so it will never be incorrectly marked as expired.
    let current_block = match get_recent_block(runtime).await {
        Ok(block) => block,
        Err(e) => {
            log!(Priority::Info, "Failed to get current block: {e}");
            return;
        }
    };

    let signatures: Vec<Signature> = all_transactions.keys().copied().collect();
    let statuses = check_transaction_statuses(runtime, signatures).await;

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

    for signature in &statuses.not_found {
        if !is_blockhash_expired(all_transactions[signature], current_block.block_height) {
            continue;
        }
        if read_state(|state| state.is_sweep_transaction(signature)) {
            log!(
                Priority::Error,
                "Sweep transaction {signature} expired, dropping its deposits"
            );
        } else {
            log!(
                Priority::Info,
                "Transaction {signature} expired, marking for resubmission"
            );
        }
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

    if !more_to_process {
        // All work fits in this round
        scopeguard::ScopeGuard::into_inner(reschedule);
    }
}

fn is_blockhash_expired(
    transaction_block_height: BlockHeight,
    current_block_height: BlockHeight,
) -> bool {
    current_block_height.saturating_sub(transaction_block_height) > MAX_BLOCKHASH_AGE_IN_BLOCKS
}

/// Resubmit transactions that have been marked for resubmission by
/// [`finalize_transactions`].
pub async fn resubmit_transactions<R: CanisterRuntime>(runtime: R) {
    let _guard = match TimerGuard::new(TaskType::ResubmitTransactions) {
        Ok(guard) => guard,
        Err(_) => return,
    };

    let to_resubmit: Vec<_> = read_state(|state| {
        state
            .transactions_to_resubmit()
            .iter()
            .map(|(sig, tx)| (*sig, tx.message.clone(), tx.signers.clone()))
            .collect()
    });
    if to_resubmit.is_empty() {
        return;
    }

    let more_to_process = to_resubmit.len() > MAX_CONCURRENT_RPC_CALLS;
    let reschedule = scopeguard::guard(runtime.clone(), |runtime| {
        runtime.set_timer(Duration::ZERO, resubmit_transactions);
    });

    resubmit_expired_transactions(&runtime, to_resubmit).await;

    if !more_to_process {
        // All work fits in this round
        scopeguard::ScopeGuard::into_inner(reschedule);
    }
}

/// Result of checking transaction statuses.
// Transactions that are in-flight (Processed/Confirmed) or whose status
// check failed are implicitly excluded from the below sets.
struct TransactionStatuses {
    /// Transactions confirmed as finalized on-chain without errors.
    succeeded: BTreeSet<Signature>,
    /// Transactions that finalized with an on-chain error.
    errored: BTreeMap<Signature, String>,
    /// Transactions with no on-chain status (safe to resubmit if expired).
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

async fn resubmit_expired_transactions<R: CanisterRuntime>(
    runtime: &R,
    to_resubmit: Vec<(Signature, VersionedMessage, Vec<Account>)>,
) {
    let block = match get_recent_block(runtime).await {
        Ok(block) => block,
        Err(e) => {
            log!(Priority::Info, "Failed to get recent blockhash: {e}");
            return;
        }
    };

    futures::future::join_all(to_resubmit.into_iter().take(MAX_CONCURRENT_RPC_CALLS).map(
        async |(old_signature, message, signers)| {
            match try_resubmit_transaction(runtime, old_signature, message, signers, block).await {
                Ok(new_sig) => log!(
                    Priority::Info,
                    "Resubmitted transaction {old_signature} as {new_sig}"
                ),
                Err(e) => log!(
                    Priority::Info,
                    "Failed to resubmit transaction {old_signature}: {e}"
                ),
            }
        },
    ))
    .await;
}

async fn try_resubmit_transaction<R: CanisterRuntime>(
    runtime: &R,
    old_signature: Signature,
    versioned_message: VersionedMessage,
    signers: Vec<Account>,
    block: Block,
) -> Result<Signature, ResubmitError> {
    let VersionedMessage::Legacy(mut message) = versioned_message;
    message.recent_blockhash = block.blockhash;

    let mut transaction = Transaction::new_unsigned(message);
    transaction.signatures = sign_bytes(
        signers.iter().map(derivation_path),
        &runtime.signer(),
        transaction.message_data(),
    )
    .await?;

    let new_signature = transaction.signatures[0];

    mutate_state(|state| {
        process_event(
            state,
            EventType::ResubmittedTransaction {
                old_signature,
                new_signature,
                new_block_height: block.block_height,
            },
            runtime,
        )
    });

    submit_transaction(runtime, transaction).await?;

    Ok(new_signature)
}

#[derive(Debug, Error)]
enum ResubmitError {
    #[error("failed to submit new transaction: {0}")]
    Submit(#[from] SubmitTransactionError),
    #[error("failed to sign transaction: {0}")]
    Signing(#[from] SignCallError),
}
