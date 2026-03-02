use crate::{
    address::get_deposit_address,
    guard::update_balance_guard,
    ledger::mint,
    runtime::CanisterRuntime,
    state::{
        audit::process_event,
        event::{AcceptedDepositEvent, EventType},
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

    if let Some(deposit_status) = read_state(|state| state.deposit_status(&(account, signature))) {
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

    if deposit_amount < read_state(|state| state.minimum_deposit_amount()) {
        return Err(UpdateBalanceError::ValueTooSmall);
    }

    let deposit_event = AcceptedDepositEvent {
        signature,
        account,
        amount: deposit_amount,
    };
    mutate_state(|state| {
        process_event(
            state,
            EventType::AcceptedDeposit(deposit_event.clone()),
            &runtime,
        )
    });

    // TODO DEFI-2643: If minting fails, we should try again later automatically (i.e. set up a
    //  timer that checks events to mint.
    // TODO DEFI-2643: Handle the case where the timer execution triggers while we are awaiting the
    //  response from the ledger and we concurrently try to mint for the same `AcceptedDeposit`
    //  event, i.e. watch out for race conditions!
    // TODO DEFI-2643: Handle the case where the mint calls panic with a scopeguard, similar to the
    //  ckBTC minter.
    match mint(&runtime, deposit_event).await {
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
