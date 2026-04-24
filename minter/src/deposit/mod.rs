use sol_rpc_types::Lamport;
use solana_address::Address;
use solana_transaction_status_client_types::{
    EncodedConfirmedTransactionWithStatusMeta, UiTransactionError,
};
use thiserror::Error;

#[cfg(test)]
mod tests;

pub mod automatic;
pub mod manual;

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
