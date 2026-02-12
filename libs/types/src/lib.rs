//! Candid types used by the Candid interface of the ckSOL minter.

#![forbid(unsafe_code)]
#![forbid(missing_docs)]

pub use sol_rpc_types::{Lamport, Pubkey as Address};

use candid::{CandidType, Principal};
use icrc_ledger_types::icrc1::account::Subaccount;
use serde::{Deserialize, Serialize};
use std::fmt;

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
