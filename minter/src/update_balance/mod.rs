use crate::{
    address::get_deposit_address,
    guard::update_balance_guard,
    ledger::mint,
    runtime::CanisterRuntime,
    state::read_state,
    transaction::{get_deposit_amount_to_address, try_get_transaction},
};
use canlog::log;
use cksol_types::{DepositStatus, UpdateBalanceError};
use cksol_types_internal::log::Priority;
use icrc_ledger_types::icrc1::account::Account;

#[cfg(test)]
mod tests;

pub async fn update_balance<R: CanisterRuntime>(
    runtime: R,
    account: Account,
    signature: solana_signature::Signature,
) -> Result<DepositStatus, UpdateBalanceError> {
    // TODO DEFI-2643: Check state to see if transaction is known
    let _guard = update_balance_guard(account)?;

    let maybe_transaction = try_get_transaction(&runtime, signature)
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

    let deposit_address = get_deposit_address(account).await;
    let deposit_amount =
        get_deposit_amount_to_address(transaction, deposit_address).map_err(|e| {
            log!(
                Priority::Info,
                "Error parsing deposit transaction with signature {signature}: {e}"
            );
            UpdateBalanceError::InvalidDepositTransaction(e.to_string())
        })?;

    if deposit_amount < read_state(|state| state.minimum_deposit_amount()) {
        return Err(UpdateBalanceError::ValueTooSmall);
    }
    let amount_to_mint = deposit_amount - read_state(|state| state.deposit_fee());

    // TODO DEFI-2643: Record event for processed deposit

    match mint(&runtime, account, amount_to_mint, signature.into()).await {
        Ok(deposit_status) => Ok(deposit_status),
        Err(e) => {
            log!(
                Priority::Info,
                "Error minting tokens for deposit transaction with signature {signature}: {e}"
            );
            Ok(DepositStatus::Processing(signature.into()))
        }
    }
}
