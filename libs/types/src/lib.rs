//! Candid types used by the Candid interface of the ckSOL minter.

#![forbid(unsafe_code)]
#![forbid(missing_docs)]

use candid::{CandidType, Principal};
use icrc_ledger_types::icrc1::account::Subaccount;
use serde::{Deserialize, Serialize};
pub use sol_rpc_types::Pubkey as Address;

/// Arguments for a request to the `getDepositAddress` ckSOL minter endpoint.
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

/// Arguments for a request to the `retrieve_sol` ckSOL minter endpoint.
#[derive(Clone, Eq, PartialEq, Debug, Default, CandidType, Deserialize, Serialize)]
pub struct RetrieveSolArgs {
    /// The subaccount to burn ckSOL from.
    pub from_subaccount: Option<Subaccount>,

    /// Amount to retrieve in Lamports.
    pub amount: u64,

    /// Address where to send Solana tokens.
    pub address: String,
}

/// The successful result of calling the `retrieve_sol` endpoint.
#[derive(Clone, Eq, PartialEq, Debug, CandidType, Deserialize)]
pub struct RetrieveSolOk {
    /// The index of the burn block on the ckSOL ledger
    pub block_index: u64,
}

/// The error result of calling the `retrieve_sol` endpoint.
#[derive(Clone, Eq, PartialEq, Debug, CandidType, Deserialize)]
pub enum RetrieveSolError {
    /// There is another request for this principal.
    AlreadyProcessing,

    /// The withdrawal amount is too low.
    /// The returned 
    AmountTooLow(u64),

    /// The Solana address is not valid.
    MalformedAddress(String),

    /// The withdrawal account does not hold the requested ckSOL amount.
    InsufficientFunds {
        /// The current balance of the withdrawal account.
        balance: u64,
    },

    /// There are too many concurrent requests, retry later.
    TemporarilyUnavailable(String),

    /// A generic error reserved for future extensions.
    GenericError {
        /// Generic error message.
        error_message: String,
        /// Generic error code.
        error_code: u64,
    },
}
