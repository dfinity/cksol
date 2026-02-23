use crate::{runtime::CanisterRuntime, state::read_state};
use cksol_types::UpdateBalanceError;
use ic_canister_runtime::IcError;
use sol_rpc_types::{CommitmentLevel, GetTransactionEncoding, Lamport, MultiRpcResult, RpcError};
use solana_address::Address;
use solana_transaction_status_client_types::EncodedConfirmedTransactionWithStatusMeta;
use thiserror::Error;

#[cfg(test)]
mod tests;

// The amount of cycles we attach for a single `getTransaction` call to the SOL RPC canister.
// TODO DEFI-2643: Move this to `State` and set during init/upgrade.
const CYCLES_TO_ATTACH_FOR_GET_TRANSACTION: u128 = 1_000_000_000_000;

pub async fn try_get_transaction<R: CanisterRuntime>(
    runtime: R,
    signature: solana_signature::Signature,
) -> Result<Option<EncodedConfirmedTransactionWithStatusMeta>, GetTransactionError> {
    // TODO DEFI-2643: Make sure caller has sufficiently many cycles attached
    let result = read_state(|state| state.sol_rpc_client(runtime.inter_canister_call_runtime()))
        .get_transaction(signature)
        .with_encoding(GetTransactionEncoding::Base64)
        .with_commitment(CommitmentLevel::Finalized)
        .with_cycles(CYCLES_TO_ATTACH_FOR_GET_TRANSACTION)
        .try_send()
        .await;
    // TODO DEFI-2643: Take (cost of call to SOL RPC canister + overhead) cycles from caller
    match result.map_err(GetTransactionError::IcError)? {
        MultiRpcResult::Consistent(Ok(maybe_transaction)) => Ok(maybe_transaction),
        MultiRpcResult::Consistent(Err(e)) => Err(GetTransactionError::RpcError(e)),
        MultiRpcResult::Inconsistent(_) => Err(GetTransactionError::InconsistentRpcResults),
    }
}

#[derive(Debug, PartialEq, Error)]
pub enum GetTransactionError {
    #[error("Error while calling SOL RPC canister: {0}")]
    IcError(IcError),
    #[error("RPC error while fetching transaction: {0}")]
    RpcError(RpcError),
    #[error("Inconsistent RPC results for transaction")]
    InconsistentRpcResults,
}

impl From<GetTransactionError> for UpdateBalanceError {
    fn from(error: GetTransactionError) -> Self {
        UpdateBalanceError::TemporarilyUnavailable(error.to_string())
    }
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

    let deposit_address_index = account_keys
        .iter()
        .position(|address| address == &deposit_address)
        .ok_or(GetDepositAmountError::DepositAddressNotInAccountKeys)?;

    // The deposit address must be writable (to receive funds) but must not
    // be a signer (it's controlled by the minter, not the depositor).
    if !message.is_maybe_writable(deposit_address_index, None) {
        return Err(GetDepositAmountError::DepositAddressNotWriteable);
    }
    if message.is_signer(deposit_address_index) {
        return Err(GetDepositAmountError::DepositAddressSigner);
    }

    let meta = transaction
        .transaction
        .meta
        .ok_or(GetDepositAmountError::NoMetaField)?;
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
    #[error("Deposit address must not be a signer")]
    DepositAddressSigner,
    #[error("'getTransaction' RPC response has no 'meta' field")]
    NoMetaField,
    #[error("Invalid transaction: {0}")]
    TransactionParsingFailed(String),
}
