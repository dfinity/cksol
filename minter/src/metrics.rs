use crate::state::{QuarantineCause, State};
use crate::storage::{self, FailedCreditReason, FailedMintReason};
use ic_metrics_encoder::MetricsEncoder;

const WASM_PAGE_SIZE_IN_BYTES: usize = 65536;
const NANOS_PER_SECOND: u64 = 1_000_000_000;

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
    w.encode_gauge(
        "oldest_incomplete_withdrawal_age_seconds",
        age_seconds(s.oldest_incomplete_withdrawal_created_at()).metric_value(),
        "Age of the oldest incomplete withdrawal request in seconds. Returns 0 if there are no incomplete withdrawals.",
    )?;
    w.encode_gauge(
        "oldest_in_flight_deposit_age_seconds",
        age_seconds(s.deposits().oldest_in_flight_queued_at()).metric_value(),
        "Age of the oldest in-flight deposit in seconds, from queued until minted. Returns 0 if there are no in-flight deposits.",
    )?;
    w.encode_gauge(
        "oldest_pending_mint_age_seconds",
        age_seconds(s.deposits().oldest_pending_mint_created_at()).metric_value(),
        "Age of the oldest pending ckSOL mint in seconds, from the created_at_time the ledger deduplicates it by, counting up to the deduplication window. Returns 0 if there are no pending mints.",
    )?;
    let mut failed_credit_attempts = w.counter_vec(
        "failed_credit_attempts",
        "Number of failed attempts to credit the deposits of a finalized sweep, by reason.",
    )?;
    for reason in FailedCreditReason::ALL {
        failed_credit_attempts = failed_credit_attempts.value(
            &[("reason", reason.label())],
            storage::failed_credit_attempt_count(reason).metric_value(),
        )?;
    }
    let mut failed_mint_attempts = w.counter_vec(
        "failed_mint_attempts",
        "Number of failed attempts to mint a pending deposit on the ckSOL ledger, by reason.",
    )?;
    for reason in FailedMintReason::ALL {
        failed_mint_attempts = failed_mint_attempts.value(
            &[("reason", reason.label())],
            storage::failed_mint_attempt_count(reason).metric_value(),
        )?;
    }
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
    Ok(())
}

fn age_seconds(start: Option<u64>) -> u64 {
    start
        .map(|start| ic_cdk::api::time().saturating_sub(start) / NANOS_PER_SECOND)
        .unwrap_or(0)
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
