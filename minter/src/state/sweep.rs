//! Validation of a fetched sweep transaction against its plan.
use crate::state::{SweepMismatch, SweepSettlementError, UnreadableOutcome};
use solana_hash::Hash;
use solana_message::{Message, VersionedMessage};
use solana_transaction::versioned::VersionedTransaction;
use solana_transaction_status_client_types::UiTransactionStatusMeta;

/// Checks that a transaction succeeded with the planned message and complete lamport balances.
pub(crate) fn validate_sweep_transaction<'a>(
    transaction: &'a VersionedTransaction,
    meta: Option<&'a UiTransactionStatusMeta>,
    planned_message: impl FnOnce(Hash) -> Message,
) -> Result<(&'a Message, &'a UiTransactionStatusMeta), SweepSettlementError> {
    let meta = meta.ok_or(UnreadableOutcome::NoMetaField)?;
    if let Some(error) = &meta.err {
        return Err(SweepMismatch::TransactionFailed {
            error: error.to_string(),
        }
        .into());
    }
    let VersionedMessage::Legacy(message) = &transaction.message else {
        return Err(SweepMismatch::UnexpectedMessage.into());
    };
    if message != &planned_message(message.recent_blockhash) {
        return Err(SweepMismatch::UnexpectedMessage.into());
    }
    if meta.pre_balances.len() != message.account_keys.len()
        || meta.post_balances.len() != message.account_keys.len()
    {
        return Err(UnreadableOutcome::IncompleteBalances.into());
    }
    Ok((message, meta))
}
