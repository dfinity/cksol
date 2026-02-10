//! Candid types used by the Candid interface of the ckSOL minter.

#![forbid(unsafe_code)]
#![forbid(missing_docs)]

use candid::{CandidType, Principal};
use icrc_ledger_types::icrc1::account::Subaccount;
use serde::{Deserialize, Serialize};
pub use sol_rpc_types::Pubkey as Address;
use sol_rpc_types::Signature;
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
    ///
    /// If not set, defaults to the caller's principal.
    /// The resolved owner must be a non-anonymous principal.
    pub owner: Option<Principal>,
    /// The subaccount for which to update the balance.
    pub subaccount: Option<Subaccount>,
    /// Signature of the deposit transaction.
    pub signature: Signature,
}

/// An error from the `update_balance` ckSOL minter endpoint.
#[derive(Debug, Clone, PartialEq, CandidType, Deserialize, Error)]
pub enum UpdateBalanceError {
    /// A transient error occurred while fetching the Solana transaction for
    /// the given signature.
    #[error("An transient error occurred while fetching the transaction")]
    TransientRpcError,
    /// No matching transaction was found for the given signature.
    ///
    /// This can also happen if the transaction is not yet finalized.
    #[error("No transaction found for the given signature")]
    TransactionNotFound,
    /// The Solana transaction with the given signature is not a valid
    /// deposit to the owner's deposit address.
    #[error("Invalid deposit to the owner's address: {0}")]
    InvalidDepositTransaction(InvalidDepositTransaction),
}

/// The transaction for the given signature is not a valid deposit.
#[derive(Debug, Clone, PartialEq, CandidType, Deserialize, Error)]
pub enum InvalidDepositTransaction {
    /// Failed to decode transaction.
    #[error("Failed to decode transaction")]
    DecodingFailed,
    /// The transaction is not a valid transfer to the deposit address .
    #[error("Transaction not a transfer to the deposit address: {0}")]
    InvalidTransfer(String),
    /// The deposit amount is below the minimum deposit threshold.
    #[error("Insufficient deposit amount, received: {received}, minimum: {minimum}")]
    InsufficientDepositAmount {
        /// The received deposit amount in lamports.
        received: u64,
        /// The minimum deposit amount in lamports.
        minimum: u64,
    },
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
