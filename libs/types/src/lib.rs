//! Candid types used by the Candid interface of the ckSOL minter.

#![forbid(unsafe_code)]
#![forbid(missing_docs)]

use candid::{CandidType, Principal};
use icrc_ledger_types::icrc1::account::Subaccount;
use serde::{Deserialize, Serialize};
pub use sol_rpc_types::Pubkey as Address;
use sol_rpc_types::{
    EncodedConfirmedTransactionWithStatusMeta, RpcError, RpcResult, RpcSource, Signature,
};
use thiserror::Error;
use std::fmt;

/// Arguments for a request to the `get_deposit_address` ckSOL minter endpoint.
#[derive(Clone, Eq, PartialEq, Debug, Default, CandidType, Deserialize, Serialize)]
pub struct GetDepositAddressArgs {
    /// The principal to deposit funds to.
    ///
    /// If not set, defaults to the caller's principal.
    /// The resolved owner must be a non-anonymous principal.
    pub owner: Option<Principal>,
    /// The subaccount to deposit funds to.
    pub subaccount: Option<Subaccount>,
}

/// Arguments for a request to the `update_balance` ckSOL minter endpoint.
#[derive(Clone, Eq, PartialEq, Debug, CandidType, Deserialize, Serialize)]
pub struct UpdateBalanceArgs {
    /// If provided, update the balance for this principal.
    /// Otherwise, update the balance for the caller.
    pub owner: Option<Principal>,
    /// If provided, update the balance for this subaccount.
    /// Otherwise, the default subaccount will be used.
    pub subaccount: Option<Subaccount>,
    /// Signature of the deposit transaction.
    pub signature: Signature,
}

/// An error from the `updateBalance` ckSOL minter endpoint.
#[derive(Debug, Clone, PartialEq, CandidType, Deserialize, Error)]
pub enum UpdateBalanceError {
    /// An error occurred while getting the transaction with the SOL RPC canister.
    #[error("An error occurred while getting the transaction: {0}")]
    GetTransactionError(GetTransactionError),
    /// No transaction found for the given signature.
    #[error("No transaction found with the given signature")]
    TransactionNotFound,
    /// The transaction for the given signature is invalid.
    #[error("Failed to decode transaction: {0}")]
    InvalidTransaction(InvalidTransaction),
}

/// An error occurred while getting the transaction with the SOL RPC canister.
#[derive(Debug, Clone, PartialEq, CandidType, Deserialize, Error)]
pub enum GetTransactionError {
    /// An IC error occurred while calling the SOL RPC canister.
    #[error("An IC error occurred while calling the SOL RPC canister: {0:?}")]
    IcError(String),
    /// An RPC error occurred while calling the SOL RPC canister.
    #[error("An RPC error occurred while calling the SOL RPC canister: {0:?}")]
    RpcError(RpcError),
    /// The SOL RPC canister returned inconsistent results.
    #[error("The SOL RPC canister returned inconsistent results: {0:?}")]
    InconsistentResults(
        Vec<(
            RpcSource,
            RpcResult<Option<EncodedConfirmedTransactionWithStatusMeta>>,
        )>,
    ),
}

/// The transaction for the given signature is invalid.
#[derive(Debug, Clone, PartialEq, CandidType, Deserialize, Error)]
pub enum InvalidTransaction {
    /// Failed to decode transaction.
    #[error("Failed to decode transaction")]
    DecodingFailed,
    /// Transaction does not have a `meta` field. This might be because it is not confirmed.
    #[error("No transaction meta")]
    NoTransactionMeta,
    /// Deposit address not part of transaction.
    #[error("Deposit address not part of transaction")]
    NotDepositToAddress,
}

/// The ID of one of the ICP root keys.
/// See the [tEdDSA documentation](https://internetcomputer.org/docs/building-apps/network-features/signatures/t-schnorr#signing-messages-and-transactions)
/// for more details.
#[derive(Clone, Eq, PartialEq, Debug, Default, CandidType, Deserialize, Serialize)]
pub enum Ed25519KeyName {
    /// Only available on the local development environment started by `dfx`.
    #[default]
    LocalDevelopment,
    /// Test key available on the ICP mainnet.
    MainnetTestKey1,
    /// Production key available on the ICP mainnet.
    MainnetProdKey1,
}

impl fmt::Display for Ed25519KeyName {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::LocalDevelopment => write!(f, "dfx_test_key"),
            Self::MainnetTestKey1 => write!(f, "test_key_1"),
            Self::MainnetProdKey1 => write!(f, "key_1"),
        }
    }
}
