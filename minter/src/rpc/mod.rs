use crate::{
    constants::{GET_SIGNATURE_STATUSES_CYCLES, MAX_HTTP_OUTCALL_RESPONSE_BYTES},
    runtime::CanisterRuntime,
    state::read_state,
};
use cksol_types::UpdateBalanceForTransactionError;
use derive_more::From;
use ic_canister_runtime::IcError;
use sol_rpc_types::{CommitmentLevel, GetTransactionEncoding, MultiRpcResult, RpcError, Slot};
use solana_hash::Hash;
use solana_signature::Signature;
use solana_transaction::Transaction;
use solana_transaction_status_client_types::EncodedConfirmedTransactionWithStatusMeta;
use thiserror::Error;

#[cfg(test)]
mod tests;

pub async fn get_transaction<R: CanisterRuntime>(
    runtime: &R,
    signature: Signature,
    cycles_to_attach: u128,
) -> Result<Option<EncodedConfirmedTransactionWithStatusMeta>, GetTransactionError> {
    let result = read_state(|state| state.sol_rpc_client(runtime.inter_canister_call_runtime()))
        .get_transaction(signature)
        .with_encoding(GetTransactionEncoding::Base64)
        .with_commitment(CommitmentLevel::Finalized)
        .with_response_size_estimate(MAX_HTTP_OUTCALL_RESPONSE_BYTES)
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

impl From<GetTransactionError> for UpdateBalanceForTransactionError {
    fn from(error: GetTransactionError) -> Self {
        UpdateBalanceForTransactionError::TemporarilyUnavailable(error.to_string())
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

pub async fn get_recent_slot_and_blockhash<R: CanisterRuntime>(
    runtime: &R,
) -> Result<(Slot, Hash), GetRecentBlockhashError> {
    let client = read_state(|state| state.sol_rpc_client(runtime.inter_canister_call_runtime()));
    match client.get_recent_block().try_send().await {
        Ok((slot, block)) => {
            let blockhash: Hash =
                block
                    .blockhash
                    .parse()
                    .map_err(|e: solana_hash::ParseHashError| {
                        GetRecentBlockhashError::Failed(vec![e.to_string()])
                    })?;
            Ok((slot, blockhash))
        }
        Err(errors) => Err(GetRecentBlockhashError::Failed(
            errors.into_iter().map(|e| e.to_string()).collect(),
        )),
    }
}

#[derive(Debug, PartialEq, Error)]
pub enum GetRecentBlockhashError {
    #[error("Failed to get recent block: {0:?}")]
    Failed(Vec<String>),
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
        .with_response_size_estimate(MAX_HTTP_OUTCALL_RESPONSE_BYTES)
        .with_cycles(GET_SIGNATURE_STATUSES_CYCLES)
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
