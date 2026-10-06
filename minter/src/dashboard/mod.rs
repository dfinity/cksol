use crate::{
    address::minter_address,
    state::{QuarantineCause, State},
};
use askama::Template;
use candid::Principal;
use cksol_types_internal::SolanaNetwork;
use ic_http_types::HttpRequest;
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
    pub quarantined_swept_deposits_start: usize,
    pub minted_sweeps_start: usize,
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
            quarantined_swept_deposits_start: parse(req, "quarantined_swept_deposits_start")?,
            minted_sweeps_start: parse(req, "minted_sweeps_start")?,
            withdrawals_start: parse(req, "withdrawals_start")?,
        })
    }

    /// Returns a query string fragment with all pagination params except `exclude`.
    fn other_params(&self, exclude: &str) -> String {
        [
            (
                "quarantined_swept_deposits_start",
                self.quarantined_swept_deposits_start,
            ),
            ("minted_sweeps_start", self.minted_sweeps_start),
            ("withdrawals_start", self.withdrawals_start),
        ]
        .into_iter()
        .filter(|(name, _)| *name != exclude)
        .map(|(name, value)| format!("&{name}={value}"))
        .collect()
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
        Self::from_rows(
            items.iter(),
            Clone::clone,
            current_page_offset,
            page_size,
            num_cols,
            table_reference,
            page_offset_query_param,
            other_query_params,
        )
    }

    /// Paginates without materializing the whole table: only the rows of the current page
    /// go through `to_row`, so rendering one page stays constant in the number of rows.
    #[allow(clippy::too_many_arguments)]
    pub fn from_rows<I, F>(
        rows: I,
        to_row: F,
        current_page_offset: usize,
        page_size: usize,
        num_cols: usize,
        table_reference: &str,
        page_offset_query_param: &str,
        other_query_params: String,
    ) -> Self
    where
        I: ExactSizeIterator,
        F: Fn(I::Item) -> T,
    {
        let total_items = rows.len();

        // Align offset to page boundary and clamp to the last valid page.
        let offset = if page_size == 0 || total_items == 0 {
            0
        } else {
            let aligned = (current_page_offset / page_size) * page_size;
            let max_start = ((total_items - 1) / page_size) * page_size;
            aligned.min(max_start)
        };

        Self {
            current_page: rows.skip(offset).take(page_size).map(to_row).collect(),
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
    pub burned_amount: String,
    pub burn_block_index: String,
    pub status: &'static str,
}

/// A deposit of a finalized sweep whose outcome did not match its plan, shown with the
/// amount the sweep planned to move so that an operator can resolve it manually.
#[derive(Clone)]
pub struct DashboardQuarantinedDeposit {
    pub deposit_id: String,
    pub account: String,
    pub signature: String,
    pub cause: &'static str,
    pub planned_amount: String,
    /// The mint the deposit is still owed, or `-` when the sweep was never credited.
    pub amount_to_mint: String,
}

/// A swept deposit whose ckSOL mint landed on the ledger.
#[derive(Clone)]
pub struct DashboardMintedSweep {
    pub deposit_id: String,
    pub account: String,
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
    pub withdrawal_fee: String,
    pub minimum_deposit_amount: String,
    pub minimum_withdrawal_amount: String,
    pub balance: String,
    pub quarantined_swept_deposits_table: DashboardPaginatedTable<DashboardQuarantinedDeposit>,
    pub minted_sweeps_table: DashboardPaginatedTable<DashboardMintedSweep>,
    pub withdrawals_table: DashboardPaginatedTable<DashboardWithdrawal>,
}

impl DashboardTemplate {
    pub fn from_state(state: &State, pagination: DashboardPaginationParameters) -> Self {
        let minter_address = state
            .minter_public_key()
            .map(|key| minter_address(key).to_string())
            .unwrap_or_default();

        let quarantined_swept_deposits: Vec<DashboardQuarantinedDeposit> = state
            .deposits()
            .quarantined()
            .iter()
            .rev()
            .map(|(deposit_id, quarantined)| DashboardQuarantinedDeposit {
                deposit_id: deposit_id.to_string(),
                account: quarantined.account().to_string(),
                signature: quarantined.sweep_signature().to_string(),
                cause: match quarantined.cause {
                    QuarantineCause::SweepUnreadable => "Sweep unreadable",
                    QuarantineCause::MintUnresolved { .. } => "Mint unresolved",
                },
                planned_amount: lamports_to_sol(quarantined.planned_amount()),
                amount_to_mint: match quarantined.cause {
                    QuarantineCause::SweepUnreadable => "-".to_string(),
                    QuarantineCause::MintUnresolved { amount_to_mint, .. } => {
                        lamports_to_sol(amount_to_mint)
                    }
                },
            })
            .collect();
        let quarantined_swept_deposits_table = DashboardPaginatedTable::from_items(
            &quarantined_swept_deposits,
            pagination.quarantined_swept_deposits_start,
            DEFAULT_PAGE_SIZE,
            6,
            "quarantined-swept-deposits",
            "quarantined_swept_deposits_start",
            pagination.other_params("quarantined_swept_deposits_start"),
        );

        let minted_sweeps_table = DashboardPaginatedTable::from_rows(
            state.deposits().minted().iter().rev(),
            |(deposit_id, minted)| DashboardMintedSweep {
                deposit_id: deposit_id.to_string(),
                account: minted.deposit.deposit.account.to_string(),
                minted_amount: lamports_to_sol(minted.minted_amount),
                mint_block_index: minted.mint_block_index.to_string(),
            },
            pagination.minted_sweeps_start,
            DEFAULT_PAGE_SIZE,
            4,
            "minted-sweeps",
            "minted_sweeps_start",
            pagination.other_params("minted_sweeps_start"),
        );

        let mut withdrawals: Vec<DashboardWithdrawal> = Vec::new();

        fn push_withdrawal(
            withdrawals: &mut Vec<DashboardWithdrawal>,
            burn_index: &crate::numeric::LedgerBurnIndex,
            req: &crate::state::event::WithdrawalRequest,
            status: &'static str,
            transaction: Option<String>,
        ) {
            withdrawals.push(DashboardWithdrawal {
                transaction,
                account: req.account.to_string(),
                withdrawal_amount: lamports_to_sol(req.amount_to_transfer),
                burned_amount: lamports_to_sol(req.burned_amount),
                burn_block_index: burn_index.to_string(),
                status,
            });
        }

        // Pending and sent (active) newest-first, then finalized (succeeded/failed) newest-first.
        for (burn_index, pending) in state
            .pending_withdrawal_requests()
            .iter()
            .rev()
            .chain(state.created_withdrawal_requests().iter().rev())
        {
            push_withdrawal(
                &mut withdrawals,
                burn_index,
                &pending.request,
                "Pending",
                None,
            );
        }
        for (burn_index, sent) in state.sent_withdrawal_requests().iter().rev() {
            push_withdrawal(
                &mut withdrawals,
                burn_index,
                &sent.request,
                "Sent",
                Some(sent.signature.to_string()),
            );
        }
        for (burn_index, sent) in state.successful_withdrawal_requests().iter().rev() {
            push_withdrawal(
                &mut withdrawals,
                burn_index,
                &sent.request,
                "Succeeded",
                Some(sent.signature.to_string()),
            );
        }
        for (burn_index, sent) in state.failed_withdrawal_requests().iter().rev() {
            push_withdrawal(
                &mut withdrawals,
                burn_index,
                &sent.request,
                "Failed",
                Some(sent.signature.to_string()),
            );
        }

        let withdrawals_table = DashboardPaginatedTable::from_items(
            &withdrawals,
            pagination.withdrawals_start,
            DEFAULT_PAGE_SIZE,
            6,
            "withdrawals",
            "withdrawals_start",
            pagination.other_params("withdrawals_start"),
        );

        let network = state.solana_network();
        DashboardTemplate {
            solana_cluster: format!("{:?}", network),
            solscan_suffix: solscan_cluster_suffix(network),
            minter_address,
            ledger_canister_id: state.ledger_canister_id(),
            sol_rpc_canister_id: state.sol_rpc_canister_id(),
            master_key_name: state.master_key_name().to_string(),
            withdrawal_fee: lamports_to_sol(state.withdrawal_fee()),
            minimum_deposit_amount: lamports_to_sol(state.minimum_deposit_amount()),
            minimum_withdrawal_amount: lamports_to_sol(state.minimum_withdrawal_amount()),
            balance: lamports_to_sol(state.balance()),
            quarantined_swept_deposits_table,
            minted_sweeps_table,
            withdrawals_table,
        }
    }
}
