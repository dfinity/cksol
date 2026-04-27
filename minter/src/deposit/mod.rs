use crate::{
    address::{account_address, lazy_get_schnorr_master_key},
    rpc::get_transaction,
    runtime::CanisterRuntime,
    state::{event::DepositId, read_state},
};
use canlog::log;
use cksol_types::ProcessDepositError;
use cksol_types_internal::log::Priority;
use icrc_ledger_types::icrc1::account::Account;
use sol_rpc_types::Lamport;
use solana_address::Address;
use solana_signature::Signature;
use solana_transaction_status_client_types::{
    EncodedConfirmedTransactionWithStatusMeta, UiTransactionError,
};
use thiserror::Error;

#[cfg(test)]
mod tests;

pub mod automatic;
pub mod manual;

pub async fn fetch_and_validate_deposit<R: CanisterRuntime>(
    runtime: &R,
    account: Account,
    signature: Signature,
    fee: Lamport,
) -> Result<(DepositId, Lamport, Lamport), ProcessDepositError> {
    let deposit_id = DepositId { account, signature };
    let master_key = lazy_get_schnorr_master_key(runtime).await;
    let deposit_address = account_address(&master_key, &account);

    let maybe_transaction = get_transaction(runtime, signature).await.map_err(|e| {
        log!(
            Priority::Info,
            "Error fetching transaction for deposit {deposit_id:?}: {e}"
        );
        ProcessDepositError::from(e)
    })?;

    let transaction = match maybe_transaction {
        Some(t) => t,
        None => return Err(ProcessDepositError::TransactionNotFound),
    };

    let deposit_amount =
        get_deposit_amount_to_address(transaction, deposit_address).map_err(|e| {
            log!(
                Priority::Info,
                "Error parsing deposit transaction with signature {signature}: {e}"
            );
            ProcessDepositError::InvalidDepositTransaction(e.to_string())
        })?;

    let minimum_deposit_amount = read_state(|s| s.minimum_deposit_amount());
    if deposit_amount < minimum_deposit_amount {
        return Err(ProcessDepositError::ValueTooSmall {
            minimum_deposit_amount,
            deposit_amount,
        });
    }

    let amount_to_mint = deposit_amount
        .checked_sub(fee)
        .expect("BUG: deposit amount is less than fee");

    Ok((deposit_id, deposit_amount, amount_to_mint))
}

pub fn get_deposit_amount_to_address(
    transaction: EncodedConfirmedTransactionWithStatusMeta,
    deposit_address: Address,
) -> Result<Lamport, GetDepositAmountError> {
    let message = transaction
        .transaction
        .transaction
        .decode()
        .ok_or(GetDepositAmountError::TransactionParsingFailed(
            "Transaction decoding failed".to_string(),
        ))?
        .message;

    // Search only static account keys, which guarantees the deposit address
    // is sourced from the transaction itself (not an address lookup table).
    let account_keys = message.static_account_keys();

    // The deposit transaction must transfer funds to the deposit address, meaning
    // the deposit address must be in the account keys and it must be writable.
    let deposit_address_index = account_keys
        .iter()
        .position(|address| address == &deposit_address)
        .ok_or(GetDepositAmountError::DepositAddressNotInAccountKeys)?;
    if !message.is_maybe_writable(deposit_address_index, None) {
        return Err(GetDepositAmountError::DepositAddressNotWriteable);
    }

    // The deposit address must not be a signer (it's controlled by the minter, not the depositor).
    assert!(
        !message.is_signer(deposit_address_index),
        "Deposit address must not be a signer!"
    );

    let meta = transaction
        .transaction
        .meta
        .ok_or(GetDepositAmountError::NoMetaField)?;

    // Sanity check: a failed transaction shouldn't affect post balances, but
    // we reject it explicitly rather than relying on that invariant.
    if let Some(err) = meta.err {
        return Err(GetDepositAmountError::TransactionFailed(err));
    }

    let pre_balance = *meta.pre_balances.get(deposit_address_index).ok_or(
        GetDepositAmountError::TransactionParsingFailed(
            "Index out of bounds for pre-balances".to_string(),
        ),
    )?;
    let post_balance = *meta.post_balances.get(deposit_address_index).ok_or(
        GetDepositAmountError::TransactionParsingFailed(
            "Index out of bounds for post-balances".to_string(),
        ),
    )?;

    Ok(post_balance.saturating_sub(pre_balance))
}

#[derive(Debug, PartialEq, Error)]
pub enum GetDepositAmountError {
    #[error("Transaction must target deposit address")]
    DepositAddressNotInAccountKeys,
    #[error("Deposit address must be writable")]
    DepositAddressNotWriteable,
    #[error("'getTransaction' RPC response has no 'meta' field")]
    NoMetaField,
    #[error("Transaction failed: {0}")]
    TransactionFailed(UiTransactionError),
    #[error("Invalid transaction: {0}")]
    TransactionParsingFailed(String),
}
