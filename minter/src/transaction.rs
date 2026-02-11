use crate::state::read_state;
use cksol_types::UpdateBalanceError;
use ic_canister_runtime::Runtime;
use sol_rpc_client::SolRpcClient;
use sol_rpc_types::{CommitmentLevel, GetTransactionEncoding, Lamport, MultiRpcResult};
use solana_address::Address;
use solana_transaction_status_client_types::EncodedConfirmedTransactionWithStatusMeta;

pub async fn try_get_transaction(
    runtime: impl Runtime,
    signature: solana_signature::Signature,
) -> Result<EncodedConfirmedTransactionWithStatusMeta, UpdateBalanceError> {
    let result = SolRpcClient::builder(runtime, read_state(|state| state.sol_rpc_canister_id))
        .build()
        .get_transaction(signature)
        .with_encoding(GetTransactionEncoding::Base64)
        .with_commitment(CommitmentLevel::Finalized)
        .with_cycles(10_000_000_000_000) // TODO: Re-try strategy?
        .try_send()
        .await
        .map_err(|e| {
            UpdateBalanceError::TemporarilyUnavailable(format!(
                "Error while calling SOL RPC canister: {e:?}"
            ))
        })?;
    match result {
        MultiRpcResult::Consistent(Ok(Some(transaction))) => Ok(transaction),
        MultiRpcResult::Consistent(Ok(None)) => Err(UpdateBalanceError::TransactionNotFound),
        MultiRpcResult::Consistent(Err(e)) => Err(UpdateBalanceError::TemporarilyUnavailable(
            format!("RPC error while fetching transaction {signature}: {e:?}"),
        )),
        MultiRpcResult::Inconsistent(_) => Err(UpdateBalanceError::TemporarilyUnavailable(
            format!("Inconsistent RPC results for transaction {signature}"),
        )),
    }
}

pub fn get_deposit_amount_to_address(
    transaction: EncodedConfirmedTransactionWithStatusMeta,
    deposit_address: Address,
) -> Result<Lamport, String> {
    let message = transaction
        .transaction
        .transaction
        .decode()
        .ok_or("Transaction decoding failed".to_string())?
        .message;

    // Search only static account keys, which guarantees the deposit address
    // is sourced from the transaction itself (not an address lookup table).
    let account_keys = message.static_account_keys();

    let deposit_address_index = account_keys
        .iter()
        .position(|address| address == &deposit_address)
        .ok_or("Deposit address not part of transaction account keys".to_string())?;

    // The deposit address must be writable (to receive funds) but must not
    // be a signer (it's controlled by the minter, not the depositor).
    if !message.is_maybe_writable(deposit_address_index, None) {
        return Err("Deposit address must be writable".to_string());
    }
    if message.is_signer(deposit_address_index) {
        return Err("Deposit address cannot be a signer".to_string());
    }

    let meta = transaction
        .transaction
        .meta
        .ok_or("'getTransaction' RPC response has no 'meta' field")?;
    let pre_balance = *meta
        .pre_balances
        .get(deposit_address_index)
        .ok_or("Deposit address index out of bounds for pre balances")?;
    let post_balance = *meta
        .post_balances
        .get(deposit_address_index)
        .ok_or("Deposit address index out of bounds for post balances")?;

    Ok(post_balance.saturating_sub(pre_balance))
}
