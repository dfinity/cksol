use crate::{
    constants::{
        GET_BALANCE_CYCLES, GET_RECENT_BLOCK_MAX_TRIES, GET_SIGNATURE_STATUSES_CYCLES,
        GET_TRANSACTION_CYCLES, MAX_HTTP_OUTCALL_RESPONSE_BYTES,
    },
    runtime::CanisterRuntime,
    state::read_state,
};
use cksol_types::{DepositSolError, ProcessDepositError};
use derive_more::From;
use ic_canister_runtime::IcError;
use minicbor::{Decode, Encode};
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

pub async fn get_transaction<R: CanisterRuntime>(
    runtime: &R,
    signature: Signature,
) -> Result<Option<EncodedConfirmedTransactionWithStatusMeta>, GetTransactionError> {
    let result = read_state(|state| state.sol_rpc_client(runtime.inter_canister_call_runtime()))
        .get_transaction(signature)
        .with_encoding(GetTransactionEncoding::Base64)
        .with_commitment(CommitmentLevel::Finalized)
        .with_max_supported_transaction_version(0)
        .with_response_size_estimate(MAX_HTTP_OUTCALL_RESPONSE_BYTES)
        .with_cycles(GET_TRANSACTION_CYCLES)
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

impl From<GetTransactionError> for ProcessDepositError {
    fn from(error: GetTransactionError) -> Self {
        ProcessDepositError::TemporarilyUnavailable(error.to_string())
    }
}

pub async fn get_balance<R: CanisterRuntime>(
    runtime: &R,
    address: Address,
) -> Result<Lamport, GetBalanceError> {
    let result = read_state(|state| state.sol_rpc_client(runtime.inter_canister_call_runtime()))
        .get_balance(address)
        .with_commitment(CommitmentLevel::Finalized)
        .with_cycles(GET_BALANCE_CYCLES)
        .try_send()
        .await;
    match result? {
        MultiRpcResult::Consistent(Ok(balance)) => Ok(balance),
        MultiRpcResult::Consistent(Err(e)) => Err(GetBalanceError::RpcError(e)),
        MultiRpcResult::Inconsistent(_) => Err(GetBalanceError::InconsistentRpcResults),
    }
}

#[derive(Debug, PartialEq, Error, From)]
pub enum GetBalanceError {
    #[error("Error while calling SOL RPC canister: {0}")]
    IcError(IcError),
    #[error("RPC error while fetching balance: {0}")]
    RpcError(RpcError),
    #[error("Inconsistent RPC results for balance")]
    InconsistentRpcResults,
}

impl From<GetBalanceError> for DepositSolError {
    fn from(error: GetBalanceError) -> Self {
        DepositSolError::TemporarilyUnavailable(error.to_string())
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

pub async fn get_recent_block<R: CanisterRuntime>(
    runtime: &R,
) -> Result<Block, GetRecentBlockError> {
    let client = read_state(|state| state.sol_rpc_client(runtime.inter_canister_call_runtime()));
    match client
        .get_recent_block()
        .with_num_tries(GET_RECENT_BLOCK_MAX_TRIES)
        .try_send()
        .await
    {
        Ok((slot, block)) => {
            let blockhash: Hash =
                block
                    .blockhash
                    .parse()
                    .map_err(|e: solana_hash::ParseHashError| {
                        GetRecentBlockError::Failed(vec![e.to_string()])
                    })?;
            let block_height = block
                .block_height
                .map(BlockHeight::from)
                .ok_or(GetRecentBlockError::MissingBlockHeight { slot })?;
            Ok(Block {
                slot,
                blockhash,
                block_height,
            })
        }
        Err(errors) => Err(GetRecentBlockError::Failed(
            errors.into_iter().map(|e| e.to_string()).collect(),
        )),
    }
}

/// A block whose blockhash a new transaction can use.
///
/// The blockhash stays valid for 150 blocks after `block_height`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Block {
    pub slot: Slot,
    pub blockhash: Hash,
    pub block_height: BlockHeight,
}

/// The height of a block in the Solana ledger, i.e. the number of blocks
/// beneath it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Decode, Encode, From)]
#[cbor(transparent)]
pub struct BlockHeight(#[n(0)] u64);

impl BlockHeight {
    pub const fn new(height: u64) -> Self {
        Self(height)
    }

    pub const fn get(self) -> u64 {
        self.0
    }

    pub fn saturating_sub(self, other: Self) -> Self {
        Self(self.0.saturating_sub(other.0))
    }
}

#[derive(Debug, PartialEq, Error)]
pub enum GetRecentBlockError {
    #[error("Failed to get recent block: {0:?}")]
    Failed(Vec<String>),
    #[error("Block at slot {slot} has no block height")]
    MissingBlockHeight { slot: Slot },
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
