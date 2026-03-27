use crate::state::State;
use askama::Template;
use candid::Principal;

#[cfg(test)]
mod tests;

const LAMPORTS_PER_SOL: u64 = 1_000_000_000;

pub fn lamports_to_sol(lamports: u64) -> String {
    let whole = lamports / LAMPORTS_PER_SOL;
    let frac = lamports % LAMPORTS_PER_SOL;
    if frac == 0 {
        format!("{whole}")
    } else {
        let frac_str = format!("{:09}", frac).trim_end_matches('0').to_string();
        format!("{whole}.{frac_str}")
    }
}

#[derive(Template)]
#[template(path = "dashboard.html")]
pub struct DashboardTemplate {
    pub solana_cluster: String,
    pub minter_address: String,
    pub ledger_canister_id: Principal,
    pub sol_rpc_canister_id: Principal,
    pub master_key_name: String,
    pub deposit_fee: String,
    pub withdrawal_fee: String,
    pub minimum_deposit_amount: String,
    pub minimum_withdrawal_amount: String,
}

impl DashboardTemplate {
    pub fn from_state(state: &State) -> Self {
        let minter_address = state
            .minter_public_key()
            .map(|key| {
                crate::address::derive_public_key(key, vec![])
                    .serialize_raw()
                    .into()
            })
            .map(|addr: solana_address::Address| addr.to_string())
            .unwrap_or_default();

        DashboardTemplate {
            solana_cluster: format!("{:?}", state.solana_network()),
            minter_address,
            ledger_canister_id: state.ledger_canister_id(),
            sol_rpc_canister_id: state.sol_rpc_canister_id(),
            master_key_name: state.master_key_name().to_string(),
            deposit_fee: lamports_to_sol(state.deposit_fee()),
            withdrawal_fee: lamports_to_sol(state.withdrawal_fee()),
            minimum_deposit_amount: lamports_to_sol(state.minimum_deposit_amount()),
            minimum_withdrawal_amount: lamports_to_sol(state.minimum_withdrawal_amount()),
        }
    }
}
