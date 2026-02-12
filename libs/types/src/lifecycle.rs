//! Types used for lifecycle management of the ckSOL minter canister.
//!
//! Types in this module are unstable and breaking changes do not break the canister API.

use candid::CandidType;
use serde::{Deserialize, Serialize};
use std::fmt;

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
