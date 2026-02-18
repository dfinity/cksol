use crate::{address, ledger, state::read_state, transaction, transaction::try_get_transaction};
use canlog::log;
use cksol_types::{DepositStatus, UpdateBalanceError};
use cksol_types_internal::log::Priority;
use ic_canister_runtime::Runtime;
use icrc_ledger_types::icrc1::account::Account;

#[cfg(test)]
mod tests;

pub async fn update_balance<R: Runtime + Clone>(
    runtime: R,
    account: Account,
    signature: solana_signature::Signature,
) -> Result<DepositStatus, UpdateBalanceError> {
    let deposit_address = address::get_deposit_address(account).await;

    let maybe_transaction = try_get_transaction(runtime.clone(), signature)
        .await
        .map_err(|e| {
            log!(
                Priority::Info,
                "Error fetching transaction with signature {signature}: {e}"
            );
            UpdateBalanceError::from(e)
        })?;

    let transaction = match maybe_transaction {
        Some(transaction) => Ok(transaction),
        None => Err(UpdateBalanceError::TransactionNotFound),
    }?;

    let deposit_amount = transaction::get_deposit_amount_to_address(transaction, deposit_address)
        .map_err(|e| {
        log!(
            Priority::Info,
            "Error parsing deposit transaction with signature {signature}: {e}"
        );
        UpdateBalanceError::InvalidDepositTransaction(e.to_string())
    })?;

    let deposit_fee = read_state(|state| state.deposit_fee());
    if deposit_amount < deposit_fee {
        return Err(UpdateBalanceError::ValueTooSmall);
    }
    let mint_amount = deposit_amount - deposit_fee;

    ledger::mint(runtime, account, mint_amount, signature.into()).await
}
