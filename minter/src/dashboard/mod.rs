use crate::{address::minter_address, runtime::CanisterRuntime, state::State};
use askama::Template;
use candid::Principal;
use cksol_types_internal::SolanaNetwork;
use ic_http_types::HttpRequest;
use std::cmp::Reverse;
use std::str::FromStr;

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

fn solscan_cluster_suffix(network: SolanaNetwork) -> &'static str {
    match network {
        SolanaNetwork::Mainnet => "",
        SolanaNetwork::Devnet => "?cluster=devnet",
        SolanaNetwork::Testnet => "?cluster=testnet",
    }
}

#[cfg(test)]
mod tests;

pub(crate) const DEFAULT_PAGE_SIZE: usize = 100;

// --- Pagination ---

#[derive(Default, Clone)]
pub struct DashboardPaginationParameters {
    pub minted_deposits_start: usize,
    pub withdrawals_start: usize,
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
            withdrawals_start: parse(req, "withdrawals_start")?,
        })
    }
}

#[derive(Clone)]
pub struct DashboardPaginatedTable<T> {
    pub current_page: Vec<T>,
    pub pagination: DashboardTablePagination,
    total_items: usize,
}

impl<T: Clone> DashboardPaginatedTable<T> {
    pub fn from_items(
        items: &[T],
        current_page_offset: usize,
        page_size: usize,
        num_cols: usize,
        table_reference: &str,
        page_offset_query_param: &str,
        other_query_params: String,
    ) -> Self {
        let total_items = items.len();

        // Align offset to page boundary and clamp to the last valid page.
        let offset = if page_size == 0 || total_items == 0 {
            0
        } else {
            let aligned = (current_page_offset / page_size) * page_size;
            let max_start = ((total_items - 1) / page_size) * page_size;
            aligned.min(max_start)
        };

        Self {
            current_page: items.iter().skip(offset).take(page_size).cloned().collect(),
            pagination: DashboardTablePagination::new(
                total_items,
                offset,
                page_size,
                num_cols,
                table_reference,
                page_offset_query_param,
                other_query_params,
            ),
            total_items,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.total_items == 0
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
    pub other_query_params: String,
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
        other_query_params: String,
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
            other_query_params,
            table_width,
            current_page_index: current_offset / page_size + 1,
            pages,
        }
    }
}

// --- Dashboard data ---

#[derive(Clone)]
pub struct DashboardWithdrawal {
    pub transaction: Option<String>,
    pub account: String,
    pub withdrawal_amount: String,
    pub burnt_amount: String,
    pub burn_block_index: String,
    pub status: &'static str,
}

#[derive(Clone)]
pub struct DashboardMintedDeposit {
    pub signature: String,
    pub account: String,
    pub deposit_amount: String,
    pub minted_amount: String,
    pub mint_block_index: String,
}

#[derive(Template)]
#[template(path = "dashboard.html")]
pub struct DashboardTemplate {
    pub solana_cluster: String,
    pub solscan_suffix: &'static str,
    pub minter_address: String,
    pub ledger_canister_id: Principal,
    pub sol_rpc_canister_id: Principal,
    pub master_key_name: String,
    pub deposit_fee: String,
    pub withdrawal_fee: String,
    pub minimum_deposit_amount: String,
    pub minimum_withdrawal_amount: String,
    pub balance: String,
    pub minted_deposits_table: DashboardPaginatedTable<DashboardMintedDeposit>,
    pub withdrawals_table: DashboardPaginatedTable<DashboardWithdrawal>,
}

impl DashboardTemplate {
    pub fn from_state<R: CanisterRuntime>(
        state: &State,
        runtime: &R,
        pagination: DashboardPaginationParameters,
    ) -> Self {
        let minter_address = state
            .minter_public_key()
            .map(|key| minter_address(key, runtime).to_string())
            .unwrap_or_default();

        let mut minted_deposits: Vec<_> = state
            .minted_deposits()
            .iter()
            .map(|(deposit_id, minted)| {
                (
                    *minted.block_index.get(),
                    DashboardMintedDeposit {
                        signature: deposit_id.signature.to_string(),
                        account: deposit_id.account.to_string(),
                        deposit_amount: lamports_to_sol(minted.deposit.deposit_amount),
                        minted_amount: lamports_to_sol(minted.deposit.amount_to_mint),
                        mint_block_index: minted.block_index.to_string(),
                    },
                )
            })
            .collect();
        minted_deposits.sort_unstable_by_key(|(block_index, _)| Reverse(*block_index));
        let minted_deposits: Vec<_> = minted_deposits.into_iter().map(|(_, d)| d).collect();

        let minted_deposits_table = DashboardPaginatedTable::from_items(
            &minted_deposits,
            pagination.minted_deposits_start,
            DEFAULT_PAGE_SIZE,
            5,
            "minted-deposits",
            "minted_deposits_start",
            format!("&withdrawals_start={}", pagination.withdrawals_start),
        );

        let mut withdrawals: Vec<_> = Vec::new();

        fn push_withdrawal(
            withdrawals: &mut Vec<(u64, DashboardWithdrawal)>,
            burn_index: &crate::numeric::LedgerBurnIndex,
            req: &crate::state::event::WithdrawSolRequest,
            status: &'static str,
            transaction: Option<String>,
        ) {
            let transfer_amount = req.withdrawal_amount.saturating_sub(req.withdrawal_fee);
            withdrawals.push((
                *burn_index.get(),
                DashboardWithdrawal {
                    transaction,
                    account: req.account.to_string(),
                    withdrawal_amount: lamports_to_sol(transfer_amount),
                    burnt_amount: lamports_to_sol(req.withdrawal_amount),
                    burn_block_index: burn_index.to_string(),
                    status,
                },
            ));
        }

        for (burn_index, req) in state.pending_withdrawal_requests() {
            push_withdrawal(&mut withdrawals, burn_index, req, "Pending", None);
        }
        for (burn_index, sent) in state.sent_withdrawal_requests() {
            push_withdrawal(
                &mut withdrawals,
                burn_index,
                &sent.request,
                "Sent",
                Some(sent.signature.to_string()),
            );
        }
        for (burn_index, sent) in state.successful_withdrawal_requests() {
            push_withdrawal(
                &mut withdrawals,
                burn_index,
                &sent.request,
                "Succeeded",
                Some(sent.signature.to_string()),
            );
        }
        for (burn_index, sent) in state.failed_withdrawal_requests() {
            push_withdrawal(
                &mut withdrawals,
                burn_index,
                &sent.request,
                "Failed",
                Some(sent.signature.to_string()),
            );
        }
        withdrawals.sort_unstable_by_key(|(burn_index, _)| Reverse(*burn_index));
        let withdrawals: Vec<_> = withdrawals.into_iter().map(|(_, w)| w).collect();

        let withdrawals_table = DashboardPaginatedTable::from_items(
            &withdrawals,
            pagination.withdrawals_start,
            DEFAULT_PAGE_SIZE,
            6,
            "withdrawals",
            "withdrawals_start",
            format!(
                "&minted_deposits_start={}",
                pagination.minted_deposits_start
            ),
        );

        let network = state.solana_network();
        DashboardTemplate {
            solana_cluster: format!("{:?}", network),
            solscan_suffix: solscan_cluster_suffix(network),
            minter_address,
            ledger_canister_id: state.ledger_canister_id(),
            sol_rpc_canister_id: state.sol_rpc_canister_id(),
            master_key_name: state.master_key_name().to_string(),
            deposit_fee: lamports_to_sol(state.deposit_fee()),
            withdrawal_fee: lamports_to_sol(state.withdrawal_fee()),
            minimum_deposit_amount: lamports_to_sol(state.minimum_deposit_amount()),
            minimum_withdrawal_amount: lamports_to_sol(state.minimum_withdrawal_amount()),
            balance: lamports_to_sol(state.balance()),
            minted_deposits_table,
            withdrawals_table,
        }
    }
}
