use crate::state::read_state;
use cksol_types::UpdateBalanceError;
use ic_canister_runtime::{IcError, Runtime};
use sol_rpc_client::SolRpcClient;
use sol_rpc_types::{
    CommitmentLevel, ConsensusStrategy, GetTransactionEncoding, MultiRpcResult, RpcError,
    RpcSources, SolanaCluster,
};
use solana_transaction_status_client_types::EncodedConfirmedTransactionWithStatusMeta;
use thiserror::Error;

#[cfg(test)]
mod tests;

// The amount of cycles we attach for a single `getTransaction` call to the SOL RPC canister.
// TODO DEFI-2643: Move this to `State` and set during init/upgrade.
const CYCLES_TO_ATTACH_FOR_GET_TRANSACTION: u128 = 1_000_000_000_000;

pub async fn try_get_transaction<R: Runtime>(
    runtime: R,
    signature: solana_signature::Signature,
) -> Result<Option<EncodedConfirmedTransactionWithStatusMeta>, GetTransactionError> {
    // TODO DEFI-2643: Make sure caller has sufficiently many cycles attached
    let result = sol_rpc_client(runtime)
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

fn sol_rpc_client<R: Runtime>(runtime: R) -> SolRpcClient<R> {
    // The maximum size of an HTTPs outcall response is 2MB:
    // https://docs.internetcomputer.org/references/ic-interface-spec#ic-http_request
    const MAX_RESPONSE_BYTES: u64 = 2_000_000;

    SolRpcClient::builder(runtime, read_state(|state| state.sol_rpc_canister_id()))
        .with_rpc_sources(RpcSources::Default(SolanaCluster::Mainnet))
        .with_response_size_estimate(MAX_RESPONSE_BYTES)
        .with_consensus_strategy(ConsensusStrategy::Threshold {
            min: 3,
            total: Some(4),
        })
        .build()
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
