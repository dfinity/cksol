//! Candid types used by the Candid interface of the ckSOL minter.

#![forbid(unsafe_code)]
#![forbid(missing_docs)]

use candid::{CandidType, Principal};
use icrc_ledger_types::icrc1::account::Subaccount;
use serde::{Deserialize, Serialize};
use sol_rpc_types::Lamport;
pub use sol_rpc_types::{Pubkey as Address, Signature};
use thiserror::Error;

/// The outcome of processing a Solana deposit transaction.
#[derive(Clone, Eq, PartialEq, Debug, CandidType, Deserialize, Serialize)]
pub enum DepositStatus {
    /// The deposit amount does not cover the deposit fee.
    ValueTooSmall(Signature),
    /// The transaction is a valid deposit, but the minter failed to mint ckSOL on the ledger.
    /// The caller should retry the `update_balance` call.
    Checked(Signature),
    /// The minter accepted the deposit and minted ckSOL tokens on the ledger.
    Minted {
        /// The MINT transaction index on the ledger.
        block_index: u64,
        /// The minted amount (deposit amount minus fees).
        minted_amount: Lamport,
        /// The UTXO that caused the balance update.
        signature: Signature,
    },
}

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
    /// The minter experiences temporary issues, try the call again later.
    #[error("Transient error, try the call again later: {0}")]
    TemporarilyUnavailable(String),
    /// No matching transaction was found for the given signature.
    ///
    /// This can also happen if the transaction is not yet finalized.
    #[error("No transaction found for the given signature")]
    TransactionNotFound,
    /// The Solana transaction with the given signature is not a valid
    /// deposit to the owner's deposit address.
    #[error("The transaction is not a valid deposit: {0}")]
    InvalidDepositTransaction(String),
}
