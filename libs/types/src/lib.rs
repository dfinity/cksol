//! Candid types used by the Candid interface of the ckSOL minter.

#![forbid(unsafe_code)]
#![forbid(missing_docs)]

#[cfg(test)]
mod tests;

use candid::{CandidType, Principal};
use icrc_ledger_types::icrc1::account::Subaccount;
use serde::{Deserialize, Serialize};

/// A dummy request
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, CandidType)]
pub struct DummyRequest {
    /// Input
    pub input: String,
}

/// A dummy response
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, CandidType)]
pub struct DummyResponse {
    /// Output
    pub output: String,
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
