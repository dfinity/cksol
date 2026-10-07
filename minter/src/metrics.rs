use crate::state::{QuarantineCause, State};
use crate::storage;
use ic_metrics_encoder::MetricsEncoder;

const WASM_PAGE_SIZE_IN_BYTES: usize = 65536;

pub fn encode_metrics(w: &mut MetricsEncoder<Vec<u8>>, s: &State) -> std::io::Result<()> {
    w.encode_gauge(
        "stable_memory_bytes",
        ic_cdk::stable::stable_size().metric_value() * WASM_PAGE_SIZE_IN_BYTES.metric_value(),
        "Size of the stable memory allocated by this canister.",
    )?;
    w.encode_gauge(
        "heap_memory_bytes",
        heap_memory_size_bytes().metric_value(),
        "Size of the heap memory allocated by this canister.",
    )?;
    w.gauge_vec("cycle_balance", "Cycle balance of this canister.")?
        .value(
            &[("canister", "cksol-minter")],
            ic_cdk::api::canister_cycle_balance().metric_value(),
        )?;
    w.encode_gauge(
        "canister_version",
        ic_cdk::api::canister_version().metric_value(),
        "Canister version",
    )?;
    w.encode_counter(
        "total_event_count",
        storage::total_event_count().metric_value(),
        "Total number of events in the event log.",
    )?;
    w.encode_gauge(
        "finalized_deposits",
        s.deposits().finalized().deposit_count().metric_value(),
        "Number of deposits whose sweep is finalized but not yet credited.",
    )?;
    w.encode_gauge(
        "pending_mints",
        s.deposits().pending_mints().len().metric_value(),
        "Number of swept deposits whose ckSOL mint is pending.",
    )?;
    w.encode_gauge(
        "minted_swept_deposits",
        s.deposits().minted().len().metric_value(),
        "Number of swept deposits whose ckSOL mint landed on the ledger.",
    )?;
    w.encode_gauge(
        "dropped_deposits",
        s.deposits().dropped().len().metric_value(),
        "Number of deposits whose sweep transaction failed or expired.",
    )?;
    let mut sweep_unreadable: usize = 0;
    let mut mint_unresolved: usize = 0;
    for quarantined in s.deposits().quarantined().values() {
        match quarantined.cause {
            QuarantineCause::SweepUnreadable => sweep_unreadable += 1,
            QuarantineCause::MintUnresolved { .. } => mint_unresolved += 1,
        }
    }
    w.gauge_vec(
        "quarantined_swept_deposits",
        "Number of quarantined deposits by cause: a sweep whose finalized outcome did not match its plan credited nothing, while an unresolved mint was credited but could no longer be retried and may have landed on the ledger.",
    )?
    .value(
        &[("cause", "sweep_unreadable")],
        sweep_unreadable.metric_value(),
    )?
    .value(
        &[("cause", "mint_unresolved")],
        mint_unresolved.metric_value(),
    )?;
    w.encode_gauge(
        "pending_withdrawal_requests",
        (s.pending_withdrawal_requests().len() + s.created_withdrawal_requests().len())
            .metric_value(),
        "Number of pending withdrawal requests.",
    )?;
    w.encode_gauge(
        "sent_withdrawal_requests",
        s.sent_withdrawal_requests().len().metric_value(),
        "Number of sent withdrawal requests.",
    )?;
    w.encode_gauge(
        "submitted_transactions",
        s.submitted_transactions().len().metric_value(),
        "Number of submitted Solana transactions.",
    )?;
    w.encode_gauge(
        "succeeded_transactions",
        s.succeeded_transactions().len().metric_value(),
        "Number of succeeded Solana transactions.",
    )?;
    w.encode_gauge(
        "failed_transactions",
        s.failed_transactions().len().metric_value(),
        "Number of failed Solana transactions.",
    )?;
    let oldest_incomplete_withdrawal_age_seconds = s
        .oldest_incomplete_withdrawal_created_at()
        .map(|created_at| {
            let now = ic_cdk::api::time();
            now.saturating_sub(created_at) / 1_000_000_000
        })
        .unwrap_or(0);
    w.encode_gauge(
        "oldest_incomplete_withdrawal_age_seconds",
        oldest_incomplete_withdrawal_age_seconds.metric_value(),
        "Age of the oldest incomplete withdrawal request in seconds. Returns 0 if there are no incomplete withdrawals.",
    )?;
    w.encode_gauge(
        "minter_balance",
        s.balance().metric_value(),
        "Minter balance in Lamports.",
    )?;
    w.encode_gauge(
        "post_upgrade_instructions_consumed",
        storage::with_unstable_metrics(|m| m.post_upgrade_instructions_consumed).metric_value(),
        "Number of instructions consumed during the last post-upgrade.",
    )?;
    w.encode_counter(
        "withdrawal_transaction_rebroadcasts",
        storage::with_unstable_metrics(|m| m.withdrawal_transaction_rebroadcasts).metric_value(),
        "Number of re-broadcast attempts of withdrawal transactions since the last upgrade.",
    )?;
    Ok(())
}

pub trait MetricValue {
    fn metric_value(&self) -> f64;
}

impl MetricValue for usize {
    fn metric_value(&self) -> f64 {
        *self as f64
    }
}

impl MetricValue for u64 {
    fn metric_value(&self) -> f64 {
        *self as f64
    }
}

impl MetricValue for u128 {
    fn metric_value(&self) -> f64 {
        *self as f64
    }
}

#[cfg(target_arch = "wasm32")]
fn heap_memory_size_bytes() -> usize {
    core::arch::wasm32::memory_size(0) * WASM_PAGE_SIZE_IN_BYTES
}

#[cfg(not(target_arch = "wasm32"))]
fn heap_memory_size_bytes() -> usize {
    0
}
