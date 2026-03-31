use crate::{runtime::CanisterRuntime, state::read_state};
use cksol_types::UpdateBalanceError;
use derive_more::From;
use ic_canister_runtime::IcError;
use sol_rpc_types::{
    CommitmentLevel, GetTransactionEncoding, Lamport, MultiRpcResult, RpcError, Slot,
};
use solana_address::Address;
use solana_hash::Hash;
use solana_signature::Signature;
use solana_transaction::Transaction;
use solana_transaction_status_client_types::EncodedConfirmedTransactionWithStatusMeta;
use thiserror::Error;

#[cfg(test)]
mod tests;

pub async fn try_get_transaction<R: CanisterRuntime>(
    runtime: &R,
    signature: Signature,
    cycles_to_attach: u128,
) -> Result<Option<EncodedConfirmedTransactionWithStatusMeta>, GetTransactionError> {
    let result = read_state(|state| state.sol_rpc_client(runtime.inter_canister_call_runtime()))
        .get_transaction(signature)
        .with_encoding(GetTransactionEncoding::Base64)
        .with_commitment(CommitmentLevel::Finalized)
        .with_cycles(cycles_to_attach)
        .try_send()
        .await;
    match result? {
        MultiRpcResult::Consistent(Ok(maybe_transaction)) => Ok(maybe_transaction),
        MultiRpcResult::Consistent(Err(e)) => Err(GetTransactionError::RpcError(e)),
        MultiRpcResult::Inconsistent(_) => Err(GetTransactionError::InconsistentRpcResults),
    }
}

#[derive(Debug, PartialEq, Error, From)]
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

pub async fn submit_transaction<R: CanisterRuntime>(
    runtime: &R,
    transaction: Transaction,
) -> Result<Signature, SubmitTransactionError> {
    let client = read_state(|state| state.sol_rpc_client(runtime.inter_canister_call_runtime()));
    match client.send_transaction(transaction).try_send().await {
        Ok(MultiRpcResult::Consistent(Ok(signature))) => Ok(signature),
        Ok(MultiRpcResult::Consistent(Err(e))) => Err(SubmitTransactionError::RpcError(e)),
        Ok(MultiRpcResult::Inconsistent(_)) => Err(SubmitTransactionError::InconsistentRpcResults),
        Err(e) => Err(SubmitTransactionError::IcError(e)),
    }
}

#[derive(Debug, PartialEq, Error, From)]
pub enum SubmitTransactionError {
    #[error("Error while calling SOL RPC canister: {0}")]
    IcError(IcError),
    #[error("RPC error while sending transaction: {0}")]
    RpcError(RpcError),
    #[error("Inconsistent RPC results for sendTransaction")]
    InconsistentRpcResults,
}

// TODO DEFI-2670: Update `sol_rpc_client` to return the slot along with the blockhash
//  in `estimate_recent_blockhash`, and refactor this method to return `(Hash, Slot)`.
pub async fn get_recent_blockhash<R: CanisterRuntime>(
    runtime: &R,
) -> Result<Hash, GetRecentBlockhashError> {
    let client = read_state(|state| state.sol_rpc_client(runtime.inter_canister_call_runtime()));
    match client.estimate_recent_blockhash().send().await {
        Ok(blockhash) => Ok(blockhash),
        Err(errors) => Err(GetRecentBlockhashError::Failed(
            errors.into_iter().map(|e| e.to_string()).collect(),
        )),
    }
}

#[derive(Debug, PartialEq, Error)]
pub enum GetRecentBlockhashError {
    #[error("Failed to estimate recent blockhash: {0:?}")]
    Failed(Vec<String>),
}

pub async fn get_slot<R: CanisterRuntime>(runtime: &R) -> Result<Slot, GetSlotError> {
    const MAX_RETRIES: u8 = 3;
    let client = read_state(|state| state.sol_rpc_client(runtime.inter_canister_call_runtime()));
    for _ in 0..MAX_RETRIES {
        match client.get_slot().send().await {
            MultiRpcResult::Consistent(Ok(slot)) => return Ok(slot),
            MultiRpcResult::Consistent(Err(e)) => return Err(GetSlotError::RpcError(e)),
            MultiRpcResult::Inconsistent(_) => continue,
        }
    }
    Err(GetSlotError::InconsistentRpcResults)
}

#[derive(Debug, PartialEq, Error, From)]
pub enum GetSlotError {
    #[error("RPC error while fetching slot: {0}")]
    RpcError(RpcError),
    #[error("Inconsistent RPC results for slot")]
    InconsistentRpcResults,
}

pub async fn get_signature_statuses<R: CanisterRuntime>(
    runtime: &R,
    signatures: &[Signature],
) -> Result<
    Vec<Option<solana_transaction_status_client_types::TransactionStatus>>,
    GetSignatureStatusesError,
> {
    let client = read_state(|state| state.sol_rpc_client(runtime.inter_canister_call_runtime()));
    let result = client
        .get_signature_statuses(signatures)
        .map_err(GetSignatureStatusesError::RpcError)?
        .try_send()
        .await;
    match result? {
        MultiRpcResult::Consistent(Ok(statuses)) => Ok(statuses),
        MultiRpcResult::Consistent(Err(e)) => Err(GetSignatureStatusesError::RpcError(e)),
        MultiRpcResult::Inconsistent(_) => Err(GetSignatureStatusesError::InconsistentRpcResults),
    }
}

#[derive(Debug, PartialEq, Error)]
pub enum GetSignatureStatusesError {
    #[error("Error while calling SOL RPC canister: {0}")]
    IcError(#[from] IcError),
    #[error("RPC error while fetching signature statuses: {0}")]
    RpcError(RpcError),
    #[error("Inconsistent RPC results for getSignatureStatuses")]
    InconsistentRpcResults,
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
    #[error("Invalid transaction: {0}")]
    TransactionParsingFailed(String),
}
