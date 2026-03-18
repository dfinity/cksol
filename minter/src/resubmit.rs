use crate::{
    address::derivation_path,
    guard::TimerGuard,
    runtime::CanisterRuntime,
    sol_transfer::{CreateTransferError, IcSchnorrSigner, sign_message_bytes},
    state::{TaskType, audit::process_event, event::EventType, mutate_state, read_state},
    transaction::{
        GetRecentBlockhashError, SubmitTransactionError, get_recent_blockhash, get_slot,
        submit_transaction,
    },
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

pub const RESUBMIT_TRANSACTIONS_DELAY: Duration = Duration::from_secs(60);

/// Solana blockhashes are valid for approximately 150 slots.
/// We use a slightly lower threshold to ensure we resubmit before expiration.
const BLOCKHASH_EXPIRY_SLOTS: Slot = 150;

pub async fn resubmit_transactions<R: CanisterRuntime>(runtime: R) {
    let _guard = match TimerGuard::new(TaskType::ResubmitTransactions) {
        Ok(guard) => guard,
        Err(_) => return,
    };

    let current_slot = match get_slot(&runtime).await {
        Ok(slot) => slot,
        Err(e) => {
            log!(Priority::Info, "Failed to get current slot: {e}");
            return;
        }
    };

    let expired_transactions: Vec<_> = read_state(|state| {
        state
            .submitted_transactions()
            .iter()
            .filter(|(_, tx)| current_slot.saturating_sub(tx.slot) >= BLOCKHASH_EXPIRY_SLOTS)
            .map(|(sig, tx)| (*sig, tx.message.clone(), tx.signers.clone()))
            .collect()
    });

    if expired_transactions.is_empty() {
        return;
    }

    for (signature, message, signers) in expired_transactions {
        if let Err(e) = resubmit_transaction(&runtime, signature, message, signers).await {
            log!(
                Priority::Info,
                "Failed to resubmit transaction {signature}: {e}"
            );
        }
    }
}

#[derive(Debug, Error)]
enum ResubmitError {
    #[error("failed to get recent blockhash: {0}")]
    GetBlockhash(#[from] GetRecentBlockhashError),
    #[error("failed to sign transaction: {0}")]
    Signing(#[from] CreateTransferError),
    #[error("failed to submit transaction: {0}")]
    Submit(#[from] SubmitTransactionError),
    #[error("failed to sign transaction: {0}")]
    SignError(#[from] SignCallError),
}

async fn resubmit_transaction<R: CanisterRuntime>(
    runtime: &R,
    old_signature: Signature,
    message: Message,
    signers: Vec<Account>,
) -> Result<(), ResubmitError> {
    let (new_slot, new_blockhash) = get_recent_blockhash(runtime).await?;
    let new_signature =
        resubmit_with_new_blockhash(runtime, &message, &signers, new_blockhash).await?;

    // Record the resubmission event (replaces old signature with new one and updates slot)
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

    log!(
        Priority::Info,
        "Resubmitted transaction {old_signature} with new signature {new_signature}"
    );

    Ok(())
}

/// Resubmits a transaction with a new blockhash.
async fn resubmit_with_new_blockhash<R: CanisterRuntime>(
    runtime: &R,
    original_message: &Message,
    signers: &[Account],
    new_blockhash: solana_hash::Hash,
) -> Result<Signature, ResubmitError> {
    let mut new_message = original_message.clone();
    new_message.recent_blockhash = new_blockhash;

    let mut transaction = Transaction::new_unsigned(new_message.clone());
    transaction.signatures = sign_message_bytes(
        signers.iter().map(derivation_path),
        &IcSchnorrSigner,
        transaction.message_data(),
    )
    .await?;

    let new_signature = submit_transaction(runtime, transaction).await?;
    Ok(new_signature)
}
