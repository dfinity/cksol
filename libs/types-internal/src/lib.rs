//! Types used for lifecycle management of the ckSOL minter canister.
//!
//! Types in this module are unstable and breaking changes do not break the canister API.

#![forbid(unsafe_code)]

#[cfg(feature = "log")]
pub mod log;

use candid::{CandidType, Principal};
use serde::{Deserialize, Serialize};
use sol_rpc_types::Lamport;
use std::fmt;

/// The ckSOL minter service arguments.
#[derive(Clone, Debug, CandidType, Deserialize)]
pub enum MinterArg {
    /// Initialization arguments.
    Init(InitArgs),
    /// Upgrade arguments.
    Upgrade(UpgradeArgs),
}

/// The installation args for the ckSOL minter canister.
#[derive(Clone, Debug, CandidType, Deserialize)]
pub struct InitArgs {
    /// The canister ID of the SOL RPC canister.
    pub sol_rpc_canister_id: Principal,
    /// The canister ID of the ckSOL ledger canister.
    pub ledger_canister_id: Principal,
    /// The deposit fee in lamports.
    pub deposit_fee: Lamport,
    /// The master Ed25519 key name.
    pub master_key_name: Ed25519KeyName,
}

/// The upgrade args for the ckSOL minter canister.
#[derive(Clone, Debug, CandidType, Deserialize)]
pub struct UpgradeArgs {
    /// The new deposit fee in lamports.
    pub deposit_fee: Option<Lamport>,
}

/// The ID of one of the ICP root keys.
/// See the [tEdDSA documentation](https://internetcomputer.org/docs/building-apps/network-features/signatures/t-schnorr#signing-messages-and-transactions)
/// for more details.
#[derive(Clone, Eq, PartialEq, Debug, Default, CandidType, Deserialize, Serialize)]
pub enum Ed25519KeyName {
    /// Only available on the local development environment started by `dfx`.
    LocalDevelopment,
    /// Test key available on the ICP mainnet.
    MainnetTestKey1,
    /// Production key available on the ICP mainnet.
    #[default]
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
