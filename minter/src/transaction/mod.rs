use crate::state::read_state;
use cksol_types::UpdateBalanceError;
use ic_canister_runtime::{IcError, Runtime};
use sol_rpc_client::SolRpcClient;
use sol_rpc_types::{CommitmentLevel, GetTransactionEncoding, MultiRpcResult, RpcError};
use solana_transaction_status_client_types::EncodedConfirmedTransactionWithStatusMeta;
use thiserror::Error;

#[cfg(test)]
mod tests;

pub async fn try_get_transaction<R: Runtime>(
    runtime: R,
    signature: solana_signature::Signature,
) -> Result<Option<EncodedConfirmedTransactionWithStatusMeta>, GetTransactionError> {
    let client =
        SolRpcClient::builder(runtime, read_state(|state| state.sol_rpc_canister_id())).build();
    let result = client
        .get_transaction(signature)
        .with_encoding(GetTransactionEncoding::Base64)
        .with_commitment(CommitmentLevel::Finalized)
        .with_cycles(10_000_000_000_000)
        .try_send()
        .await;
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
