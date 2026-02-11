use crate::Ed25519KeyName;
use candid::{CandidType, Deserialize, Principal};
use canlog::LogFilter;
use sol_rpc_types::Lamport;

/// The installation args for the ckSOL minter canister.
#[derive(Clone, Debug, CandidType, Deserialize)]
pub struct InstallArgs {
    /// Only log entries matching this filter will be recorded.
    /// Default is `LogFilter::ShowAll`.
    pub log_filter: Option<LogFilter>,
    /// The canister ID of the SOL RPC canister.
    pub sol_rpc_canister_id: Principal,
    /// The canister ID of the ckSOL ledger canister.
    pub ledger_canister_id: Principal,
    /// The deposit fee in lamports.
    pub deposit_fee: Lamport,
    /// The master Ed25519 key name.
    pub master_key_name: Ed25519KeyName,
}
