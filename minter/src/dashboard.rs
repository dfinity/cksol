use crate::state::State;
use askama::Template;
use candid::Principal;
use sol_rpc_types::Lamport;

#[derive(Clone)]
pub struct DashboardWithdrawalRequest {
    pub burn_index: String,
    pub from: String,
    pub to: String,
    pub amount: Lamport,
    pub fee: Lamport,
}

#[derive(Template)]
#[template(path = "dashboard.html")]
pub struct DashboardTemplate {
    pub minter_address: String,
    pub ledger_canister_id: Principal,
    pub sol_rpc_canister_id: Principal,
    pub master_key_name: String,
    pub deposit_fee: u64,
    pub withdrawal_fee: u64,
    pub minimum_deposit_amount: u64,
    pub minimum_withdrawal_amount: u64,
    pub accepted_deposits_count: usize,
    pub quarantined_deposits_count: usize,
    pub minted_deposits_count: usize,
    pub pending_withdrawal_requests_count: usize,
    pub sent_withdrawal_requests_count: usize,
    pub submitted_transactions_count: usize,
    pub deposits_to_consolidate: Vec<(String, String, Lamport)>,
    pub pending_withdrawal_requests: Vec<DashboardWithdrawalRequest>,
    pub sent_withdrawal_requests: Vec<(String, String)>,
    pub submitted_transactions: Vec<(String, u64)>,
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

        let deposits_to_consolidate: Vec<_> = state
            .deposits_to_consolidate()
            .iter()
            .map(|(mint_index, (account, amount))| {
                (mint_index.to_string(), account.to_string(), *amount)
            })
            .collect();

        let pending_withdrawal_requests: Vec<_> = state
            .pending_withdrawal_requests()
            .iter()
            .map(|(burn_index, req)| DashboardWithdrawalRequest {
                burn_index: burn_index.to_string(),
                from: req.account.to_string(),
                to: solana_address::Address::from(req.solana_address).to_string(),
                amount: req.withdrawal_amount,
                fee: req.withdrawal_fee,
            })
            .collect();

        let sent_withdrawal_requests: Vec<_> = state
            .sent_withdrawal_requests()
            .iter()
            .map(|(burn_index, sig)| (burn_index.to_string(), sig.to_string()))
            .collect();

        let submitted_transactions: Vec<_> = state
            .submitted_transactions()
            .iter()
            .map(|(sig, tx)| (sig.to_string(), tx.slot))
            .collect();

        DashboardTemplate {
            minter_address,
            ledger_canister_id: state.ledger_canister_id(),
            sol_rpc_canister_id: state.sol_rpc_canister_id(),
            master_key_name: format!("{:?}", state.master_key_name()),
            deposit_fee: state.deposit_fee(),
            withdrawal_fee: state.withdrawal_fee(),
            minimum_deposit_amount: state.minimum_deposit_amount(),
            minimum_withdrawal_amount: state.minimum_withdrawal_amount(),
            accepted_deposits_count: state.accepted_deposits().len(),
            quarantined_deposits_count: state.quarantined_deposits().len(),
            minted_deposits_count: state.minted_deposits().len(),
            pending_withdrawal_requests_count: state.pending_withdrawal_requests().len(),
            sent_withdrawal_requests_count: state.sent_withdrawal_requests().len(),
            submitted_transactions_count: state.submitted_transactions().len(),
            deposits_to_consolidate,
            pending_withdrawal_requests,
            sent_withdrawal_requests,
            submitted_transactions,
        }
    }
}
