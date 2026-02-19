use crate::transaction::try_get_transaction;
use canlog::log;
use cksol_types::{DepositStatus, UpdateBalanceError};
use cksol_types_internal::log::Priority;
use ic_canister_runtime::Runtime;
use icrc_ledger_types::icrc1::account::Account;

#[cfg(test)]
mod tests;

pub async fn update_balance<R: Runtime + Clone>(
    runtime: R,
    _account: Account,
    signature: solana_signature::Signature,
) -> Result<DepositStatus, UpdateBalanceError> {
    // TODO DEFI-2643: Add guard to prevent concurrent calls
    // TODO DEFI-2643: Check state to see if transaction is known

    let maybe_transaction = try_get_transaction(runtime.clone(), signature)
        .await
        .map_err(|e| {
            log!(
                Priority::Info,
                "Error fetching transaction with signature {signature}: {e}"
            );
            UpdateBalanceError::from(e)
        })?;

    let _transaction = match maybe_transaction {
        Some(transaction) => Ok(transaction),
        None => Err(UpdateBalanceError::TransactionNotFound),
    }?;

    // TODO DEFI-2643: Extract deposit from transaction
    Err(UpdateBalanceError::TemporarilyUnavailable(
        "Not yet implemented!".to_string(),
    ))
}
