use crate::{
    address::derivation_path,
    constants::MAX_CONCURRENT_RPC_CALLS,
    guard::TimerGuard,
    runtime::CanisterRuntime,
    signer::sign_bytes,
    state::{
        TaskType,
        audit::process_event,
        event::{EventType, VersionedMessage},
        mutate_state, read_state,
    },
    transaction::{
        SubmitTransactionError, get_recent_slot_and_blockhash, get_signature_statuses,
        submit_transaction,
    },
};
use canlog::log;
use cksol_types_internal::log::Priority;
use ic_cdk_management_canister::SignCallError;
use icrc_ledger_types::icrc1::account::Account;
use itertools::Itertools;
use sol_rpc_types::Slot;
use solana_signature::Signature;
use solana_transaction::Transaction;
use solana_transaction_status_client_types::TransactionConfirmationStatus;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;
use thiserror::Error;

#[cfg(test)]
mod tests;

pub const MONITOR_SUBMITTED_TRANSACTIONS_DELAY: Duration = Duration::from_secs(60);
const MAX_BLOCKHASH_AGE: Slot = 150;
/// Maximum number of signatures per `getSignatureStatuses` RPC call.
/// See https://solana.com/docs/rpc/http/getsignaturestatuses
const MAX_SIGNATURES_PER_STATUS_CHECK: usize = 256;

pub async fn monitor_submitted_transactions<R: CanisterRuntime>(runtime: R) {
    let _guard = match TimerGuard::new(TaskType::MonitorSubmittedTransactions) {
        Ok(guard) => guard,
        Err(_) => return,
    };

    let all_transactions: Vec<(Signature, Slot)> = read_state(|state| {
        state
            .submitted_transactions()
            .iter()
            .map(|(sig, tx)| (*sig, tx.slot))
            .collect()
    });
    if all_transactions.is_empty() {
        return;
    }

    // Fetch the current slot before checking statuses: if a transaction finalizes
    // after we snapshot the slot, the status check will see it as finalized rather
    // than missing, so it will never be incorrectly marked as expired.
    let (current_slot, _) = match get_recent_slot_and_blockhash(&runtime).await {
        Ok(result) => result,
        Err(e) => {
            log!(Priority::Info, "Failed to get current slot: {e}");
            return;
        }
    };

    let statuses = check_transaction_statuses(&runtime, &all_transactions).await;

    for (signature, error) in &statuses.errored {
        log!(
            Priority::Info,
            "Transaction {signature} finalized with error: {error}"
        );
        mutate_state(|state| {
            process_event(
                state,
                EventType::FailedTransaction {
                    signature: *signature,
                },
                &runtime,
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
                &runtime,
            )
        });
    }

    if statuses.not_found.is_empty() {
        return;
    }

    let expired_signatures: BTreeSet<Signature> = all_transactions
        .into_iter()
        .filter(|(sig, slot)| {
            statuses.not_found.contains(sig) && slot + MAX_BLOCKHASH_AGE < current_slot
        })
        .map(|(sig, _)| sig)
        .collect();

    if expired_signatures.is_empty() {
        return;
    }

    for signature in expired_signatures {
        mutate_state(|state| {
            // Skip if the transaction was finalized concurrently.
            if state.submitted_transactions().contains_key(&signature) {
                process_event(state, EventType::ExpiredTransaction { signature }, &runtime);
            }
        });
    }

    let to_resubmit: Vec<_> = read_state(|state| {
        state
            .transactions_to_resubmit()
            .iter()
            .filter_map(|sig| {
                state
                    .submitted_transactions()
                    .get(sig)
                    .map(|tx| (*sig, tx.message.clone(), tx.signers.clone()))
            })
            .collect()
    });

    resubmit_expired_transactions(&runtime, to_resubmit).await;
}

/// Result of checking transaction statuses.
struct TransactionStatuses {
    /// Transactions confirmed as finalized on-chain without errors.
    succeeded: BTreeSet<Signature>,
    /// Transactions that finalized with an on-chain error.
    errored: BTreeMap<Signature, String>,
    /// Transactions with no on-chain status (safe to resubmit if expired).
    not_found: BTreeSet<Signature>,
    // Transactions that are in-flight (Processed/Confirmed) or whose status
    // check failed are implicitly excluded — they appear in neither set.
}

async fn check_transaction_statuses<R: CanisterRuntime>(
    runtime: &R,
    transactions: &[(Signature, Slot)],
) -> TransactionStatuses {
    let signatures: Vec<Signature> = transactions.iter().map(|(sig, _)| *sig).collect();
    let batches: Vec<Vec<Signature>> = signatures
        .into_iter()
        .chunks(MAX_SIGNATURES_PER_STATUS_CHECK)
        .into_iter()
        .map(Iterator::collect)
        .collect();

    let mut result = TransactionStatuses {
        succeeded: BTreeSet::new(),
        errored: BTreeMap::new(),
        not_found: BTreeSet::new(),
    };

    for round in &batches.into_iter().chunks(MAX_CONCURRENT_RPC_CALLS) {
        let batch_results: Vec<_> = futures::future::join_all(round.map(async |batch| {
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
                        if s.confirmation_status
                            == Some(TransactionConfirmationStatus::Finalized) =>
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
    }

    result
}

async fn resubmit_expired_transactions<R: CanisterRuntime>(
    runtime: &R,
    expired: Vec<(Signature, VersionedMessage, Vec<Account>)>,
) {
    for round in &expired.into_iter().chunks(MAX_CONCURRENT_RPC_CALLS) {
        let (new_slot, new_blockhash) = match get_recent_slot_and_blockhash(runtime).await {
            Ok(result) => result,
            Err(e) => {
                log!(Priority::Info, "Failed to get recent blockhash: {e}");
                return;
            }
        };

        futures::future::join_all(round.map(async |(old_signature, message, signers)| {
            match try_resubmit_transaction(
                runtime,
                old_signature,
                message,
                signers,
                new_slot,
                new_blockhash,
            )
            .await
            {
                Ok(new_sig) => log!(
                    Priority::Info,
                    "Resubmitted transaction {old_signature} as {new_sig}"
                ),
                Err(e) => log!(
                    Priority::Info,
                    "Failed to resubmit transaction {old_signature}: {e}"
                ),
            }
        }))
        .await;
    }
}

async fn try_resubmit_transaction<R: CanisterRuntime>(
    runtime: &R,
    old_signature: Signature,
    versioned_message: VersionedMessage,
    signers: Vec<Account>,
    new_slot: Slot,
    new_blockhash: solana_hash::Hash,
) -> Result<Signature, ResubmitError> {
    let VersionedMessage::Legacy(mut message) = versioned_message;
    message.recent_blockhash = new_blockhash;

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
                new_slot,
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
