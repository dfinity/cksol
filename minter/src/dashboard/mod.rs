use crate::state::State;
use askama::Template;
use candid::Principal;
use ic_http_types::HttpRequest;
use sol_rpc_types::Lamport;
use std::cmp::Reverse;
use std::str::FromStr;

#[cfg(test)]
mod tests;

const DEFAULT_PAGE_SIZE: usize = 100;

// --- Pagination ---

#[derive(Default, Clone)]
pub struct DashboardPaginationParameters {
    pub minted_deposits_start: usize,
    pub withdrawal_transactions_start: usize,
}

impl DashboardPaginationParameters {
    pub fn from_query_params(req: &HttpRequest) -> Result<Self, String> {
        fn parse(req: &HttpRequest, param: &str) -> Result<usize, String> {
            Ok(match req.raw_query_param(param) {
                Some(arg) => usize::from_str(arg)
                    .map_err(|_| format!("failed to parse the '{param}' parameter"))?,
                None => 0,
            })
        }

        Ok(Self {
            minted_deposits_start: parse(req, "minted_deposits_start")?,
            withdrawal_transactions_start: parse(req, "withdrawal_transactions_start")?,
        })
    }
}

#[derive(Clone)]
pub struct DashboardPaginatedTable<T> {
    pub current_page: Vec<T>,
    pub pagination: DashboardTablePagination,
}

impl<T: Clone> DashboardPaginatedTable<T> {
    pub fn from_items(
        items: &[T],
        current_page_offset: usize,
        page_size: usize,
        num_cols: usize,
        table_reference: &str,
        page_offset_query_param: &str,
    ) -> Self {
        Self {
            current_page: items
                .iter()
                .skip(current_page_offset)
                .take(page_size)
                .cloned()
                .collect(),
            pagination: DashboardTablePagination::new(
                items.len(),
                current_page_offset,
                page_size,
                num_cols,
                table_reference,
                page_offset_query_param,
            ),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.current_page.is_empty()
    }

    pub fn has_more_than_one_page(&self) -> bool {
        self.pagination.pages.len() > 1
    }
}

#[derive(Clone)]
pub struct DashboardTablePage {
    pub index: usize,
    pub offset: usize,
}

#[derive(Template, Clone)]
#[template(path = "pagination.html")]
pub struct DashboardTablePagination {
    pub table_id: String,
    pub table_width: usize,
    pub page_offset_query_param: String,
    pub current_page_index: usize,
    pub pages: Vec<DashboardTablePage>,
}

impl DashboardTablePagination {
    fn new(
        num_items: usize,
        current_offset: usize,
        page_size: usize,
        table_width: usize,
        table_reference: &str,
        page_offset_query_param: &str,
    ) -> Self {
        let pages = (0..num_items)
            .step_by(page_size)
            .enumerate()
            .map(|(index, offset)| DashboardTablePage {
                index: index + 1,
                offset,
            })
            .collect();
        Self {
            table_id: String::from(table_reference),
            page_offset_query_param: String::from(page_offset_query_param),
            table_width,
            current_page_index: current_offset / page_size + 1,
            pages,
        }
    }
}

// --- Dashboard data ---

#[derive(Clone)]
pub struct DashboardMintedDeposit {
    pub signature: String,
    pub account: String,
    pub deposit_amount: Lamport,
    pub minted_amount: Lamport,
    pub mint_block_index: String,
}

#[derive(Clone)]
pub struct DashboardWithdrawalRequest {
    pub burn_index: String,
    pub from: String,
    pub to: String,
    pub amount: Lamport,
    pub fee: Lamport,
}

#[derive(Clone)]
pub struct DashboardWithdrawalTransaction {
    pub burn_index: String,
    pub to: String,
    pub amount: Lamport,
    pub signature: String,
    pub status: String,
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
    pub deposits_to_consolidate: Vec<(String, String, Lamport)>,
    pub pending_withdrawal_requests: Vec<DashboardWithdrawalRequest>,
    pub minted_deposits_table: DashboardPaginatedTable<DashboardMintedDeposit>,
    pub withdrawal_transactions_table: DashboardPaginatedTable<DashboardWithdrawalTransaction>,
}

impl DashboardTemplate {
    pub fn from_state(state: &State, pagination: DashboardPaginationParameters) -> Self {
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

        let mut minted_deposits: Vec<_> = state
            .minted_deposits()
            .iter()
            .map(|(deposit_id, minted)| DashboardMintedDeposit {
                signature: deposit_id.signature.to_string(),
                account: deposit_id.account.to_string(),
                deposit_amount: minted.deposit.deposit_amount,
                minted_amount: minted.deposit.amount_to_mint,
                mint_block_index: minted.block_index.to_string(),
            })
            .collect();
        minted_deposits.sort_unstable_by_key(|d| Reverse(d.mint_block_index.clone()));

        let minted_deposits_table = DashboardPaginatedTable::from_items(
            &minted_deposits,
            pagination.minted_deposits_start,
            DEFAULT_PAGE_SIZE,
            5,
            "minted-deposits",
            "minted_deposits_start",
        );

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

        let mut withdrawal_transactions: Vec<_> = state
            .sent_withdrawal_requests()
            .iter()
            .map(|(burn_index, sig)| {
                let sig_str = sig.to_string();
                let status = if state.succeeded_transactions().contains(sig) {
                    "Succeeded"
                } else if state.failed_transactions().contains_key(sig) {
                    "Failed"
                } else if state.submitted_transactions().contains_key(sig) {
                    "Submitted"
                } else {
                    "Unknown"
                };
                let to = state
                    .pending_withdrawal_requests()
                    .get(burn_index)
                    .map(|req| solana_address::Address::from(req.solana_address).to_string())
                    .unwrap_or_default();
                let amount = state
                    .pending_withdrawal_requests()
                    .get(burn_index)
                    .map(|req| req.withdrawal_amount)
                    .unwrap_or(0);
                DashboardWithdrawalTransaction {
                    burn_index: burn_index.to_string(),
                    to,
                    amount,
                    signature: sig_str,
                    status: status.to_string(),
                }
            })
            .collect();
        withdrawal_transactions.sort_unstable_by_key(|tx| Reverse(tx.burn_index.clone()));

        let withdrawal_transactions_table = DashboardPaginatedTable::from_items(
            &withdrawal_transactions,
            pagination.withdrawal_transactions_start,
            DEFAULT_PAGE_SIZE,
            5,
            "withdrawal-transactions",
            "withdrawal_transactions_start",
        );

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
            deposits_to_consolidate,
            pending_withdrawal_requests,
            minted_deposits_table,
            withdrawal_transactions_table,
        }
    }
}
