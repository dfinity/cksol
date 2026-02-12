use candid::{CandidType, Deserialize, Principal};
use cksol_types::{Ed25519KeyName, Lamport};

/// The installation args for the ckSOL minter canister.
#[derive(Clone, Debug, CandidType, Deserialize)]
pub struct InstallArgs {
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
pub struct UpgradeArgs {
    /// The new deposit fee in lamports.
    pub deposit_fee: Option<Lamport>,
}
