use candid::{CandidType, Deserialize, Principal};
use cksol_types::{Ed25519KeyName, Lamport};
use minicbor::{Decode, Encode};

/// The installation args for the ckSOL minter canister.
#[derive(Clone, Debug, Eq, PartialEq, CandidType, Decode, Deserialize, Encode)]
pub struct InitArgs {
    /// The canister ID of the SOL RPC canister.
    #[cbor(n(0), with = "icrc_cbor::principal")]
    pub sol_rpc_canister_id: Principal,
    /// The canister ID of the ckSOL ledger canister.
    #[cbor(n(1), with = "icrc_cbor::principal")]
    pub ledger_canister_id: Principal,
    /// The deposit fee in lamports.
    #[n(2)]
    pub deposit_fee: Lamport,
    /// The master Ed25519 key name.
    #[n(3)]
    pub master_key_name: Ed25519KeyName,
}

/// The upgrade args for the ckSOL minter canister.
#[derive(Clone, Debug, Eq, PartialEq, CandidType, Decode, Deserialize, Encode)]
pub struct UpgradeArgs {
    /// The new deposit fee in lamports.
    #[n(0)]
    pub deposit_fee: Option<Lamport>,
}
