use crate::{
    address::get_deposit_address,
    guard::update_balance_guard,
    ledger::mint,
    runtime::CanisterRuntime,
    state::{
        audit::process_event,
        event::{DepositId, EventType},
        mutate_state, read_state,
    },
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
    let _guard = update_balance_guard(account)?;

    let deposit_id = DepositId { account, signature };
    if let Some(deposit_status) = read_state(|state| state.deposit_status(&deposit_id)) {
        return Ok(deposit_status);
    }

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
    let minimum_deposit_amount = read_state(|state| state.minimum_deposit_amount());
    if deposit_amount < minimum_deposit_amount {
        return Err(UpdateBalanceError::ValueTooSmall {
            minimum_deposit_amount,
            deposit_amount,
        });
    }
    let amount_to_mint = deposit_amount
        .checked_sub(read_state(|state| state.deposit_fee()))
        .expect("BUG: deposit amount is less than deposit fee");

    mutate_state(|state| {
        process_event(
            state,
            EventType::AcceptedDeposit {
                deposit_id,
                amount_to_mint,
            },
            &runtime,
        )
    });

    match mint(&runtime, deposit_id, amount_to_mint).await {
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
