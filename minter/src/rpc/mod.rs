use crate::{
    constants::{
        GET_ACCOUNT_INFO_CYCLES, GET_BALANCE_CYCLES, GET_RECENT_BLOCK_MAX_TRIES,
        GET_SIGNATURE_STATUSES_CYCLES, GET_TRANSACTION_CYCLES, MAX_HTTP_OUTCALL_RESPONSE_BYTES,
    },
    runtime::CanisterRuntime,
    state::read_state,
};
use cksol_types::DepositSolError;
use derive_more::From;
use ic_canister_runtime::IcError;
use minicbor::{Decode, Encode};
use sol_rpc_types::{
    CommitmentLevel, GetAccountInfoEncoding, GetTransactionEncoding, Lamport, MultiRpcResult,
    RpcError, Slot,
};
use solana_account_decoder_client_types::UiAccount;
use solana_address::Address;
use solana_hash::Hash;
use solana_nonce::{state::State as NonceState, versions::Versions as NonceVersions};
use solana_sdk_ids::system_program;
use solana_signature::Signature;
use solana_transaction::{Transaction, versioned::VersionedTransaction};
use solana_transaction_status_client_types::{
    EncodedConfirmedTransactionWithStatusMeta, UiTransactionStatusMeta,
};
use thiserror::Error;

#[cfg(test)]
mod tests;

/// A `getTransaction` response attributed to the queried signature: the transaction
/// decoded by [`get_transaction`], whose first signature is the queried one and signs
/// the message.
#[derive(Debug, PartialEq)]
pub struct FetchedTransaction {
    pub transaction: VersionedTransaction,
    pub meta: Option<UiTransactionStatusMeta>,
}

pub async fn get_transaction<R: CanisterRuntime>(
    runtime: &R,
    signature: Signature,
) -> Result<Option<FetchedTransaction>, GetTransactionError> {
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
        MultiRpcResult::Consistent(Ok(Some(outcome))) => {
            let transaction = ensure_signed_with(&outcome, signature)?;
            Ok(Some(FetchedTransaction {
                transaction,
                meta: outcome.transaction.meta,
            }))
        }
        MultiRpcResult::Consistent(Ok(None)) => Ok(None),
        MultiRpcResult::Consistent(Err(e)) => Err(GetTransactionError::RpcError(e)),
        MultiRpcResult::Inconsistent(_) => Err(GetTransactionError::InconsistentRpcResults),
    }
}

fn ensure_signed_with(
    outcome: &EncodedConfirmedTransactionWithStatusMeta,
    queried: Signature,
) -> Result<VersionedTransaction, GetTransactionError> {
    let decoded = outcome
        .transaction
        .transaction
        .decode()
        .ok_or(GetTransactionError::UndecodableTransaction { queried })?;
    match decoded.signatures.first() {
        Some(first) if *first == queried => {}
        returned => {
            return Err(GetTransactionError::SignatureMismatch {
                queried,
                returned: returned.copied().map(Box::new),
            });
        }
    }
    if decoded.verify_with_results().first() != Some(&true) {
        return Err(GetTransactionError::InvalidSignature { queried });
    }
    Ok(decoded)
}

#[derive(Debug, PartialEq, Error, From)]
pub enum GetTransactionError {
    #[error("Error while calling SOL RPC canister: {0}")]
    IcError(IcError),
    #[error("RPC error while fetching transaction: {0}")]
    RpcError(RpcError),
    #[error("Inconsistent RPC results for transaction")]
    InconsistentRpcResults,
    #[error("Transaction returned for {queried} cannot be decoded")]
    #[from(ignore)]
    UndecodableTransaction { queried: Signature },
    #[error("Transaction returned for {queried} has first signature {returned:?}")]
    SignatureMismatch {
        queried: Signature,
        returned: Option<Box<Signature>>,
    },
    #[error("Transaction returned for {queried} is not signed by its fee payer")]
    #[from(ignore)]
    InvalidSignature { queried: Signature },
}

impl GetTransactionError {
    /// A consistent response that cannot be attributed to the queried signature points
    /// to a misbehaving provider or SOL RPC canister, not to a transient error.
    pub fn is_response_untrustworthy(&self) -> bool {
        match self {
            GetTransactionError::UndecodableTransaction { .. }
            | GetTransactionError::SignatureMismatch { .. }
            | GetTransactionError::InvalidSignature { .. } => true,
            GetTransactionError::IcError(_)
            | GetTransactionError::RpcError(_)
            | GetTransactionError::InconsistentRpcResults => false,
        }
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

pub async fn get_nonce_account<R: CanisterRuntime>(
    runtime: &R,
    address: Address,
) -> Result<NonceAccount, GetNonceAccountError> {
    let result = read_state(|state| state.sol_rpc_client(runtime.inter_canister_call_runtime()))
        .get_account_info(address)
        .with_encoding(GetAccountInfoEncoding::Base64)
        .with_commitment(CommitmentLevel::Finalized)
        .with_cycles(GET_ACCOUNT_INFO_CYCLES)
        .try_send()
        .await;
    match result? {
        MultiRpcResult::Consistent(Ok(Some(account))) => NonceAccount::try_from(account),
        MultiRpcResult::Consistent(Ok(None)) => Err(GetNonceAccountError::AccountNotFound),
        MultiRpcResult::Consistent(Err(e)) => Err(GetNonceAccountError::RpcError(e)),
        MultiRpcResult::Inconsistent(_) => Err(GetNonceAccountError::InconsistentRpcResults),
    }
}

/// The on-chain state of an initialized durable nonce account.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NonceAccount {
    pub authority: Address,
    pub nonce: Hash,
}

impl TryFrom<UiAccount> for NonceAccount {
    type Error = GetNonceAccountError;

    fn try_from(account: UiAccount) -> Result<Self, Self::Error> {
        if account.owner != system_program::ID.to_string() || account.executable {
            return Err(GetNonceAccountError::UnexpectedAccountMetadata {
                owner: account.owner,
                executable: account.executable,
            });
        }
        let data = account.data.decode().ok_or_else(|| {
            GetNonceAccountError::NotAnInitializedNonceAccount(
                "undecodable account data".to_string(),
            )
        })?;
        let versions: NonceVersions = bincode::deserialize(&data)
            .map_err(|e| GetNonceAccountError::NotAnInitializedNonceAccount(e.to_string()))?;
        match versions.state() {
            NonceState::Uninitialized => Err(GetNonceAccountError::NotAnInitializedNonceAccount(
                "the nonce account is uninitialized".to_string(),
            )),
            NonceState::Initialized(data) => Ok(Self {
                authority: data.authority,
                nonce: data.blockhash(),
            }),
        }
    }
}

#[derive(Debug, PartialEq, Error)]
pub enum GetNonceAccountError {
    #[error("Error while calling SOL RPC canister: {0}")]
    IcError(#[from] IcError),
    #[error("RPC error while fetching nonce account: {0}")]
    RpcError(RpcError),
    #[error("Inconsistent RPC results for getAccountInfo")]
    InconsistentRpcResults,
    #[error("Nonce account not found")]
    AccountNotFound,
    #[error(
        "Expected a non-executable account owned by the system program, got owner {owner} (executable: {executable})"
    )]
    UnexpectedAccountMetadata { owner: String, executable: bool },
    #[error("Not an initialized nonce account: {0}")]
    NotAnInitializedNonceAccount(String),
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
        .with_search_transaction_history(true)
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
