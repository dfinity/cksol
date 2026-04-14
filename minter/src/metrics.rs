use crate::state::State;
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
        "accepted_deposits",
        s.accepted_deposits().len().metric_value(),
        "Number of accepted deposits pending minting.",
    )?;
    w.encode_gauge(
        "quarantined_deposits",
        s.quarantined_deposits().len().metric_value(),
        "Number of quarantined deposits.",
    )?;
    w.encode_gauge(
        "minted_deposits",
        s.minted_deposits().len().metric_value(),
        "Number of minted deposits.",
    )?;
    w.encode_gauge(
        "deposits_to_consolidate",
        s.deposits_to_consolidate().len().metric_value(),
        "Number of deposits pending consolidation.",
    )?;
    w.encode_gauge(
        "pending_withdrawal_requests",
        s.pending_withdrawal_requests().len().metric_value(),
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
    if let Some(created_at) = s.oldest_incomplete_withdrawal_created_at() {
        let now = ic_cdk::api::time();
        let age_seconds = now.saturating_sub(created_at) / 1_000_000_000;
        w.encode_gauge(
            "oldest_incomplete_withdrawal_age_seconds",
            age_seconds.metric_value(),
            "Age of the oldest incomplete withdrawal request in seconds.",
        )?;
    }
    w.encode_gauge(
        "post_upgrade_instructions_consumed",
        storage::with_unstable_metrics(|m| m.post_upgrade_instructions_consumed).metric_value(),
        "Number of instructions consumed during the last post-upgrade.",
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
