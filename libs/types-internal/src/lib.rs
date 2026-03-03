//! Types used for lifecycle management of the ckSOL minter canister.
//!
//! Types in this module are unstable and breaking changes do not break the canister API.

#![forbid(unsafe_code)]

#[cfg(feature = "event")]
pub mod event;
#[cfg(feature = "log")]
pub mod log;

use candid::{CandidType, Principal};
use serde::{Deserialize, Serialize};
use sol_rpc_types::Lamport;
use std::fmt;

/// The ckSOL minter service arguments.
#[derive(Clone, Debug, CandidType, Deserialize)]
#[cfg_attr(feature = "event", derive(minicbor::Encode, minicbor::Decode))]
pub enum MinterArg {
    /// Initialization arguments.
    #[cfg_attr(feature = "event", n(0))]
    Init(#[cfg_attr(feature = "event", n(0))] InitArgs),
    /// Upgrade arguments.
    #[cfg_attr(feature = "event", n(1))]
    Upgrade(#[cfg_attr(feature = "event", n(0))] UpgradeArgs),
}

/// The installation args for the ckSOL minter canister.
#[derive(Clone, Eq, PartialEq, Debug, CandidType, Deserialize)]
#[cfg_attr(feature = "event", derive(minicbor::Encode, minicbor::Decode))]
pub struct InitArgs {
    /// The canister ID of the SOL RPC canister.
    #[cfg_attr(feature = "event", n(0), cbor(with = "icrc_cbor::principal"))]
    pub sol_rpc_canister_id: Principal,
    /// The canister ID of the ckSOL ledger canister.
    #[cfg_attr(feature = "event", n(1), cbor(with = "icrc_cbor::principal"))]
    pub ledger_canister_id: Principal,
    /// The deposit fee in lamports.
    #[cfg_attr(feature = "event", n(2))]
    pub deposit_fee: Lamport,
    /// The master Ed25519 key name.
    #[cfg_attr(feature = "event", n(3))]
    pub master_key_name: Ed25519KeyName,
    /// Minimum withdrawal amount in lamports.
    #[cfg_attr(feature = "event", n(4))]
    pub minimum_withdrawal_amount: Lamport,
    /// Minimum deposit amount in lamports.
    #[cfg_attr(feature = "event", n(5))]
    pub minimum_deposit_amount: Lamport,
    /// The withdrawal fee in lamports.
    #[cfg_attr(feature = "event", n(6))]
    pub withdrawal_fee: Lamport,
}

/// The upgrade args for the ckSOL minter canister.
#[derive(Clone, Default, Eq, PartialEq, Debug, CandidType, Deserialize)]
#[cfg_attr(feature = "event", derive(minicbor::Encode, minicbor::Decode))]
pub struct UpgradeArgs {
    /// The canister ID of the SOL RPC canister.
    #[cfg_attr(feature = "event", n(0), cbor(with = "icrc_cbor::principal::option"))]
    pub sol_rpc_canister_id: Option<Principal>,
    /// The new deposit fee in lamports.
    #[cfg_attr(feature = "event", n(1))]
    pub deposit_fee: Option<Lamport>,
    /// The new minimum withdrawal amount in lamports.
    #[cfg_attr(feature = "event", n(2))]
    pub minimum_withdrawal_amount: Option<Lamport>,
    /// The new minimum deposit amount in lamports.
    #[cfg_attr(feature = "event", n(3))]
    pub minimum_deposit_amount: Option<Lamport>,
    /// The new withdrawal fee in lamports.
    #[cfg_attr(feature = "event", n(4))]
    pub withdrawal_fee: Option<Lamport>,
}

/// Identifies a withdrawal request by the source ledger account and the destination Solana address.
#[derive(Clone, Eq, PartialEq, Debug, CandidType, Deserialize)]
#[cfg_attr(feature = "event", derive(minicbor::Encode, minicbor::Decode))]
pub struct WithdrawalId {
    /// The principal that initiated the withdrawal.
    #[cfg_attr(feature = "event", n(0), cbor(with = "icrc_cbor::principal"))]
    pub owner: Principal,
    /// The subaccount from which ckSOL was burned.
    #[cfg_attr(feature = "event", n(1), cbor(with = "minicbor::bytes"))]
    pub subaccount: Option<[u8; 32]>,
    /// The destination Solana address.
    #[cfg_attr(feature = "event", n(2), cbor(with = "minicbor::bytes"))]
    pub solana_address: [u8; 32],
}

/// Payload of the `WithdrawalBurned` event.
#[derive(Clone, Eq, PartialEq, Debug, CandidType, Deserialize)]
#[cfg_attr(feature = "event", derive(minicbor::Encode, minicbor::Decode))]
pub struct BurnEvent {
    /// The burn transaction index on the ckSOL ledger.
    #[cfg_attr(feature = "event", n(0))]
    pub burn_block_index: u64,
    /// The total amount burned from the user (in lamports).
    #[cfg_attr(feature = "event", n(1))]
    pub withdrawal_amount: Lamport,
    /// The fee retained by the minter (in lamports).
    #[cfg_attr(feature = "event", n(2))]
    pub withdrawal_fee: Lamport,
    /// The withdrawal request identifier.
    #[cfg_attr(feature = "event", n(3))]
    pub withdrawal_id: WithdrawalId,
}

/// The ID of one of the ICP root keys.
/// See the [tEdDSA documentation](https://internetcomputer.org/docs/building-apps/network-features/signatures/t-schnorr#signing-messages-and-transactions)
/// for more details.
#[derive(Clone, Copy, Eq, PartialEq, Debug, Default, CandidType, Deserialize, Serialize)]
#[cfg_attr(feature = "event", derive(minicbor::Encode, minicbor::Decode))]
pub enum Ed25519KeyName {
    /// Only available on the local development environment started by `dfx`.
    #[cfg_attr(feature = "event", n(0))]
    LocalDevelopment,
    /// Test key available on the ICP mainnet.
    #[cfg_attr(feature = "event", n(1))]
    MainnetTestKey1,
    /// Production key available on the ICP mainnet.
    #[default]
    #[cfg_attr(feature = "event", n(2))]
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
