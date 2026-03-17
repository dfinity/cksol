use crate::{
    address::derivation_path,
    guard::TimerGuard,
    runtime::CanisterRuntime,
    sol_transfer::{CreateTransferError, IcSchnorrSigner, SchnorrSigner},
    state::{TaskType, audit::process_event, event::EventType, mutate_state, read_state},
    transaction::{
        GetRecentBlockhashError, GetSignatureStatusError, SubmitTransactionError,
        get_recent_blockhash, get_signature_status, submit_transaction,
    },
};
use canlog::log;
use cksol_types_internal::log::Priority;
use icrc_ledger_types::icrc1::account::Account;
use solana_message::Message;
use solana_signature::Signature;
use solana_transaction::Transaction;
use std::time::Duration;
use thiserror::Error;

pub const RESUBMIT_TRANSACTIONS_DELAY: Duration = Duration::from_secs(60);

pub async fn resubmit_transactions<R: CanisterRuntime>(runtime: R) {
    let _guard = match TimerGuard::new(TaskType::ResubmitTransactions) {
        Ok(guard) => guard,
        Err(_) => return,
    };

    let submitted_transactions: Vec<_> = read_state(|state| {
        state
            .submitted_transactions()
            .iter()
            .map(|(sig, tx)| (*sig, tx.message.clone(), tx.signers.clone()))
            .collect()
    });

    if submitted_transactions.is_empty() {
        return;
    }

    for (signature, message, signers) in submitted_transactions {
        if let Err(e) = process_submitted_transaction(&runtime, signature, message, signers).await {
            log!(
                Priority::Info,
                "Failed to process submitted transaction {signature}: {e}"
            );
        }
    }
}

#[derive(Debug, Error)]
enum ResubmitError {
    #[error("failed to check transaction status: {0}")]
    GetSignatureStatus(#[from] GetSignatureStatusError),
    #[error("failed to get recent blockhash: {0}")]
    GetBlockhash(#[from] GetRecentBlockhashError),
    #[error("failed to sign transaction: {0}")]
    Signing(#[from] CreateTransferError),
    #[error("failed to submit transaction: {0}")]
    Submit(#[from] SubmitTransactionError),
}

async fn process_submitted_transaction<R: CanisterRuntime>(
    runtime: &R,
    signature: Signature,
    message: Message,
    signers: Vec<Account>,
) -> Result<(), ResubmitError> {
    // Check if the transaction has been processed using getSignatureStatuses
    let status = get_signature_status(runtime, signature).await?;

    if let Some(tx_status) = status {
        // Transaction was processed (has a slot), remove from tracking
        log!(
            Priority::Info,
            "Transaction {signature} processed at slot {}, removed from tracking",
            tx_status.slot
        );
        mutate_state(|state| state.remove_submitted_transaction(&signature));
        return Ok(());
    }

    // Transaction not found in status - likely expired or never processed
    // Resubmit with a new blockhash
    log!(
        Priority::Info,
        "Transaction {signature} not found in status, resubmitting with new blockhash"
    );

    let new_blockhash = get_recent_blockhash(runtime).await?;
    let (new_signature, new_message) =
        resubmit_with_new_blockhash(runtime, &message, &signers, new_blockhash).await?;

    // Remove the old transaction and record the new submission
    mutate_state(|state| state.remove_submitted_transaction(&signature));

    mutate_state(|state| {
        process_event(
            state,
            EventType::SubmittedTransaction {
                signature: new_signature,
                transaction: new_message,
                signers,
            },
            runtime,
        )
    });

    log!(
        Priority::Info,
        "Resubmitted transaction with new signature {new_signature}"
    );

    Ok(())
}

/// Resubmits a transaction with a new blockhash.
async fn resubmit_with_new_blockhash<R: CanisterRuntime>(
    runtime: &R,
    original_message: &Message,
    signers: &[Account],
    new_blockhash: solana_hash::Hash,
) -> Result<(Signature, Message), ResubmitError> {
    let mut new_message = original_message.clone();
    new_message.recent_blockhash = new_blockhash;

    let transaction = sign_message(&new_message, signers, &IcSchnorrSigner).await?;
    let signature = submit_transaction(runtime, transaction).await?;
    Ok((signature, new_message))
}

/// Signs a message using the provided signer and account list.
///
/// The signers must be in the same order as they appear in the transaction's
/// signature slots (i.e., the order of `message.account_keys[..num_required_signatures]`).
async fn sign_message(
    message: &Message,
    signers: &[Account],
    signer: &impl SchnorrSigner,
) -> Result<Transaction, CreateTransferError> {
    debug_assert_eq!(
        signers.len(),
        message.header.num_required_signatures as usize,
        "BUG: signers count must match num_required_signatures"
    );

    let mut transaction = Transaction::new_unsigned(message.clone());
    let message_bytes = transaction.message_data();

    // Sign with each account's derived key
    let results = futures::future::join_all(signers.iter().map(|account| {
        let dp = derivation_path(account);
        signer.sign(message_bytes.clone(), dp)
    }))
    .await;

    // Signers are stored in the same order as signature positions
    for (position, result) in results.into_iter().enumerate() {
        let signature_bytes = result?;

        let sig_bytes: [u8; 64] = signature_bytes
            .as_slice()
            .try_into()
            .expect("BUG: expected 64-byte signature");

        transaction.signatures[position] = Signature::from(sig_bytes);
    }

    Ok(transaction)
}

#[cfg(test)]
mod tests;
