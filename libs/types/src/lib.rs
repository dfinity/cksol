//! Candid types used by the Candid interface of the ckSOL minter.

#![forbid(unsafe_code)]
#![forbid(missing_docs)]

#[cfg(test)]
mod tests;

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

/// The argument to `get_sol_address` endpoint used to derive
/// the Solana address for a given `Account`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, CandidType)]
pub struct GetSolAddressArgs {
    /// The `Principal` that owns the `Account`
    pub owner: Option<Principal>,
    /// Subaccount
    pub subaccount: Option<Subaccount>,
}
