use crate::storage;
use ic_metrics_encoder::MetricsEncoder;

const WASM_PAGE_SIZE_IN_BYTES: usize = 65536;

pub fn encode_metrics(w: &mut MetricsEncoder<Vec<u8>>) -> std::io::Result<()> {
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
