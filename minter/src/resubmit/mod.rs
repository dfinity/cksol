use crate::{
    address::derivation_path,
    guard::TimerGuard,
    runtime::CanisterRuntime,
    signer::sign_bytes,
    state::{TaskType, audit::process_event, event::EventType, mutate_state, read_state},
    transaction::{SubmitTransactionError, get_recent_blockhash, get_slot, submit_transaction},
};
use canlog::log;
use cksol_types_internal::log::Priority;
use ic_cdk::management_canister::SignCallError;
use icrc_ledger_types::icrc1::account::Account;
use sol_rpc_types::Slot;
use solana_message::Message;
use solana_signature::Signature;
use solana_transaction::Transaction;
use std::time::Duration;
use thiserror::Error;

#[cfg(test)]
mod tests;

pub const RESUBMIT_TRANSACTIONS_DELAY: Duration = Duration::from_secs(60);
const MAX_BLOCKHASH_AGE: Slot = 150;
const MAX_CONCURRENT_TRANSACTIONS: usize = 10;

pub async fn resubmit_transactions<R: CanisterRuntime>(runtime: R) {
    let _guard = match TimerGuard::new(TaskType::ResubmitTransactions) {
        Ok(guard) => guard,
        Err(_) => return,
    };

    if read_state(|state| state.submitted_transactions().is_empty()) {
        return;
    }

    let current_slot = match get_slot(&runtime).await {
        Ok(slot) => slot,
        Err(e) => {
            log!(Priority::Info, "Failed to get current slot: {e}");
            return;
        }
    };
    let mut expired_transactions = read_state(|state| {
        state
            .submitted_transactions()
            .iter()
            .filter(|(_, tx)| tx.slot + MAX_BLOCKHASH_AGE < current_slot)
            .map(|(sig, tx)| (*sig, tx.message.clone(), tx.signers.clone()))
            .collect::<Vec<_>>()
    });

    while !expired_transactions.is_empty() {
        let new_blockhash = match get_recent_blockhash(&runtime).await {
            Ok(blockhash) => blockhash,
            Err(e) => {
                log!(Priority::Info, "Failed to get recent blockhash: {e}");
                return;
            }
        };
        let new_slot = match get_slot(&runtime).await {
            Ok(slot) => slot,
            Err(e) => {
                log!(Priority::Info, "Failed to get slot: {e}");
                return;
            }
        };

        let batch_size = MAX_CONCURRENT_TRANSACTIONS.min(expired_transactions.len());
        let futures = expired_transactions.drain(..batch_size).map(
            async |(old_signature, message, signers)| match resubmit_transaction_with_new_blockhash(
                &runtime,
                old_signature,
                message,
                signers,
                new_slot,
                new_blockhash,
            )
            .await
            {
                Ok(new_signature) => log!(
                    Priority::Info,
                    "Resubmitted transaction {old_signature} with new signature {new_signature}"
                ),
                Err(e) => log!(
                    Priority::Info,
                    "Failed to resubmit transaction {old_signature}: {e}"
                ),
            },
        );
        futures::future::join_all(futures).await;
    }
}

async fn resubmit_transaction_with_new_blockhash<R: CanisterRuntime>(
    runtime: &R,
    old_signature: Signature,
    message: Message,
    signers: Vec<Account>,
    new_slot: Slot,
    new_blockhash: solana_hash::Hash,
) -> Result<Signature, ResubmitError> {
    let mut message = message;
    message.recent_blockhash = new_blockhash;

    let mut transaction = Transaction::new_unsigned(message);
    transaction.signatures = sign_bytes(
        signers.iter().map(derivation_path),
        &runtime.signer(),
        transaction.message_data(),
    )
    .await?;

    let new_signature = transaction.signatures[0];

    // Record the resubmission event before submitting the transaction to ensure we don't
    // resubmit the same transaction twice in case of a panic during submission.
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
