use crate::state::State;
use askama::Template;
use candid::Principal;
use ic_http_types::HttpRequest;
use sol_rpc_types::Lamport;
use std::str::FromStr;

const DEFAULT_PAGE_SIZE: usize = 100;

// --- Pagination ---

#[derive(Default, Clone)]
pub struct DashboardPaginationParameters {
    pub submitted_transactions_start: usize,
    pub sent_withdrawal_requests_start: usize,
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
            submitted_transactions_start: parse(req, "submitted_transactions_start")?,
            sent_withdrawal_requests_start: parse(req, "sent_withdrawal_requests_start")?,
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
    pub sent_withdrawal_requests_table: DashboardPaginatedTable<(String, String)>,
    pub submitted_transactions_table: DashboardPaginatedTable<(String, u64)>,
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

        let sent_withdrawal_requests_table = DashboardPaginatedTable::from_items(
            &sent_withdrawal_requests,
            pagination.sent_withdrawal_requests_start,
            DEFAULT_PAGE_SIZE,
            2,
            "sent-withdrawal-requests",
            "sent_withdrawal_requests_start",
        );

        let submitted_transactions: Vec<_> = state
            .submitted_transactions()
            .iter()
            .map(|(sig, tx)| (sig.to_string(), tx.slot))
            .collect();

        let submitted_transactions_table = DashboardPaginatedTable::from_items(
            &submitted_transactions,
            pagination.submitted_transactions_start,
            DEFAULT_PAGE_SIZE,
            2,
            "submitted-transactions",
            "submitted_transactions_start",
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
            minted_deposits_count: state.minted_deposits().len(),
            pending_withdrawal_requests_count: state.pending_withdrawal_requests().len(),
            sent_withdrawal_requests_count: state.sent_withdrawal_requests().len(),
            submitted_transactions_count: state.submitted_transactions().len(),
            deposits_to_consolidate,
            pending_withdrawal_requests,
            sent_withdrawal_requests_table,
            submitted_transactions_table,
        }
    }
}
