use crate::state::{DepositOrigin, QuarantineCause, SplQuarantineCause, State};
use crate::storage::{
    self, FailedCreditReason, FailedMintReason, FailedSplTransferReason,
    OperatingRefillErrorReason, RearmOutcome, ScanOutcome, TopUpBalanceRead,
    UndecidedWithdrawalReason,
};
use crate::utils::timed_sized_map::Timestamp;
use cksol_types::SplRefundKind;
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
    let mut dropped_manual: usize = 0;
    let mut dropped_automatic: usize = 0;
    for dropped in s.deposits().dropped().values() {
        match dropped.deposit.origin {
            DepositOrigin::Manual => dropped_manual += 1,
            DepositOrigin::Automatic => dropped_automatic += 1,
        }
    }
    w.gauge_vec(
        "dropped_deposits",
        "Number of deposits whose sweep transaction failed or expired, by origin: queued by deposit_sol or by a balance scan of the automatic flow.",
    )?
    .value(&[("origin", "manual")], dropped_manual.metric_value())?
    .value(&[("origin", "automatic")], dropped_automatic.metric_value())?;
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
    w.encode_gauge(
        "oldest_incomplete_withdrawal_age_seconds",
        age_seconds(s.oldest_incomplete_withdrawal_created_at()).metric_value(),
        "Age of the oldest incomplete withdrawal request in seconds. Returns 0 if there are no incomplete withdrawals.",
    )?;
    w.encode_gauge(
        "oldest_in_flight_deposit_age_seconds",
        age_seconds(s.deposits().oldest_in_flight_queued_at()).metric_value(),
        "Age of the oldest in-flight deposit in seconds, from queued until minted, dropped or quarantined. Returns 0 if there are no in-flight deposits.",
    )?;
    w.encode_gauge(
        "oldest_pending_mint_age_seconds",
        age_seconds(s.deposits().oldest_pending_mint_created_at()).metric_value(),
        "Age of the oldest pending ckSOL mint in seconds, from the created_at_time the ledger deduplicates it by. Returns 0 if there are no pending mints.",
    )?;
    let now = Timestamp::from_nanos(ic_cdk::api::time());
    w.encode_gauge(
        "watched_deposit_addresses",
        s.watchlist().live_len(now).metric_value(),
        "Number of deposit addresses registered for the automatic deposit flow whose window is still open.",
    )?;
    w.encode_gauge(
        "watched_deposit_address_capacity",
        s.watchlist().capacity().metric_value(),
        "Number of live registrations the watchlist of the automatic deposit flow admits at most.",
    )?;
    w.encode_gauge(
        "oldest_due_deposit_address_scan_age_seconds",
        age_seconds(
            s.watchlist()
                .oldest_due_scan_at(now)
                .map(Timestamp::as_nanos),
        )
        .metric_value(),
        "Time in seconds since the earliest due scan of a watched deposit address that has not run yet. Returns 0 if no scan is due.",
    )?;
    w.encode_counter(
        "collected_automatic_deposit_fees",
        s.collected_automatic_deposit_fees().metric_value(),
        "Sum in Lamports of the automatic deposit fees deducted from the mints of credited sweeps.",
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
    let mut deposit_address_scans = w.counter_vec(
        "deposit_address_scans",
        "Number of balance scans of watched deposit addresses since the last upgrade, by outcome: a deposit queued, a balance below the minimum deposit amount, or a failed read.",
    )?;
    for outcome in ScanOutcome::ALL {
        deposit_address_scans = deposit_address_scans.value(
            &[("outcome", outcome.label())],
            storage::deposit_address_scan_count(outcome).metric_value(),
        )?;
    }
    let mut automatic_deposit_rearms = w.counter_vec(
        "automatic_deposit_rearms",
        "Number of automatic deposits dropped with their sweep since the last upgrade, by outcome: their account put back on the watchlist for a fresh window, or not because the watchlist was full.",
    )?;
    for outcome in RearmOutcome::ALL {
        automatic_deposit_rearms = automatic_deposit_rearms.value(
            &[("outcome", outcome.label())],
            storage::automatic_deposit_rearm_count(outcome).metric_value(),
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
    w.encode_counter(
        "withdrawal_transaction_rebroadcasts",
        storage::with_unstable_metrics(|m| m.withdrawal_transaction_rebroadcasts).metric_value(),
        "Number of re-broadcast attempts of withdrawal transactions since the last upgrade.",
    )?;
    let mut undecided_withdrawal_transactions = w.gauge_vec(
        "undecided_withdrawal_transactions",
        "Number of in-flight withdrawal transactions left undecided by the last finalization round, by reason: a stale nonce read, a failed nonce read, or a nonce advance whose outcome could not be fetched.",
    )?;
    for reason in UndecidedWithdrawalReason::ALL {
        undecided_withdrawal_transactions = undecided_withdrawal_transactions.value(
            &[("reason", reason.label())],
            storage::undecided_withdrawal_transaction_count(reason).metric_value(),
        )?;
    }
    encode_operating_balance_metrics(w, s)?;
    encode_spl_deposit_metrics(w, s)?;
    encode_spl_withdrawal_metrics(w, s, ic_cdk::api::time())?;
    Ok(())
}

pub(crate) fn encode_spl_withdrawal_metrics(
    w: &mut MetricsEncoder<Vec<u8>>,
    s: &State,
    now: u64,
) -> std::io::Result<()> {
    let oldest_age_seconds = s
        .oldest_incomplete_spl_withdrawal_created_at()
        .map(|created_at| now.saturating_sub(created_at) / NANOS_PER_SECOND)
        .unwrap_or(0);
    w.encode_gauge(
        "oldest_incomplete_spl_withdrawal_age_seconds",
        oldest_age_seconds.metric_value(),
        "Age of the oldest incomplete SPL withdrawal request in seconds, from accepted until its transaction succeeded or failed. Returns 0 if there are no incomplete SPL withdrawals.",
    )?;
    w.encode_gauge(
        "free_nonce_accounts",
        s.nonce_pool().num_free_accounts().metric_value(),
        "Number of durable nonce accounts that no withdrawal transaction uses. SOL and SPL withdrawals wait while it is 0.",
    )?;
    let mut failed_spl_withdrawals = w.counter_vec(
        "failed_spl_withdrawals",
        "Number of SPL withdrawals since the last upgrade whose transaction failed on-chain, by mint and reason: a frozen token account, a token account that does not exist, or another error.",
    )?;
    for mint in s.supported_spl_mints() {
        for reason in FailedSplTransferReason::ALL {
            failed_spl_withdrawals = failed_spl_withdrawals.value(
                &[("mint", &mint.to_string()), ("reason", reason.label())],
                storage::failed_spl_withdrawal_count(*mint, reason).metric_value(),
            )?;
        }
    }
    w.encode_counter(
        "spl_withdrawals_above_rent_reserve",
        s.spl_withdrawals_above_rent_reserve().metric_value(),
        "Number of SPL withdrawals whose destination token account cost more rent than the reserve that the user paid.",
    )?;
    w.encode_counter(
        "spl_rent_above_reserve",
        s.spl_rent_above_reserve().metric_value(),
        "Sum in Lamports of the rent that SPL withdrawals spent above their reserve. The withdrawal fee and then the operating balance pay it.",
    )?;
    let mut quarantined_spl_cksol_refunds = w.gauge_vec(
        "quarantined_spl_cksol_refunds",
        "Number of quarantined ckSOL refunds of SPL withdrawals by kind: the fee of a failed token burn, the unused rent of a successful transfer, or the rent of a failed transfer. The mint could no longer be retried and may have landed on the ledger.",
    )?;
    for (kind, label) in [
        (SplRefundKind::FailedTokenBurn, "failed_token_burn"),
        (SplRefundKind::UnusedRent, "unused_rent"),
        (SplRefundKind::FailedTransferRent, "failed_transfer_rent"),
    ] {
        let quarantined = s
            .quarantined_spl_cksol_refunds()
            .values()
            .filter(|refund| refund.kind == kind)
            .count();
        quarantined_spl_cksol_refunds =
            quarantined_spl_cksol_refunds.value(&[("kind", label)], quarantined.metric_value())?;
    }
    let mut quarantined_spl_withdrawal_reimbursements = w.gauge_vec(
        "quarantined_spl_withdrawal_reimbursements",
        "Number of failed SPL withdrawals whose token remint is quarantined, by mint. The mint could no longer be retried and may have landed on the token ledger.",
    )?;
    for mint in s.supported_spl_mints() {
        let quarantined = s
            .quarantined_spl_withdrawal_reimbursements()
            .values()
            .filter(|reimbursement| reimbursement.request.terms.mint == *mint)
            .count();
        quarantined_spl_withdrawal_reimbursements = quarantined_spl_withdrawal_reimbursements
            .value(&[("mint", &mint.to_string())], quarantined.metric_value())?;
    }
    Ok(())
}

pub(crate) fn encode_spl_deposit_metrics(
    w: &mut MetricsEncoder<Vec<u8>>,
    s: &State,
) -> std::io::Result<()> {
    let mut quarantined_spl_deposits = w.gauge_vec(
        "quarantined_spl_deposits",
        "Number of quarantined SPL deposits by mint and cause: a sweep whose finalized outcome did not match its plan credited nothing and keeps its SOL reservation, while an unresolved mint was credited but could no longer be retried and may have landed on the token ledger.",
    )?;
    for mint in s.supported_spl_mints() {
        let mut sweep_unreadable: usize = 0;
        let mut mint_unresolved: usize = 0;
        for quarantined in s.spl_deposits().quarantined().values() {
            if quarantined.deposit.deposit.mint != *mint {
                continue;
            }
            match quarantined.cause {
                SplQuarantineCause::SweepUnreadable { .. } => sweep_unreadable += 1,
                SplQuarantineCause::MintUnresolved { .. } => mint_unresolved += 1,
            }
        }
        let mint = mint.to_string();
        quarantined_spl_deposits = quarantined_spl_deposits
            .value(
                &[("mint", &mint), ("cause", "sweep_unreadable")],
                sweep_unreadable.metric_value(),
            )?
            .value(
                &[("mint", &mint), ("cause", "mint_unresolved")],
                mint_unresolved.metric_value(),
            )?;
    }
    let mut dropped_spl_deposits = w.gauge_vec(
        "dropped_spl_deposits",
        "Number of SPL deposits whose sweep transaction failed or expired, by mint.",
    )?;
    for mint in s.supported_spl_mints() {
        let dropped = s
            .spl_deposits()
            .dropped()
            .values()
            .filter(|dropped| dropped.deposit.mint == *mint)
            .count();
        dropped_spl_deposits =
            dropped_spl_deposits.value(&[("mint", &mint.to_string())], dropped.metric_value())?;
    }
    let mut failed_spl_sweeps = w.counter_vec(
        "failed_spl_sweeps",
        "Number of SPL sweeps since the last upgrade that the preflight simulation rejected or that failed on-chain, by mint and reason: a frozen token account, a token account that does not exist, or another error.",
    )?;
    for mint in s.supported_spl_mints() {
        for reason in FailedSplTransferReason::ALL {
            failed_spl_sweeps = failed_spl_sweeps.value(
                &[("mint", &mint.to_string()), ("reason", reason.label())],
                storage::failed_spl_sweep_count(*mint, reason).metric_value(),
            )?;
        }
    }
    w.encode_gauge(
        "unfunded_queued_spl_deposits",
        s.unfunded_queued_spl_deposits().metric_value(),
        "Number of queued SPL deposits whose sweep is blocked because the SOL available for SPL sweep fees does not cover it. Returns 0 if the funds cover every queued deposit.",
    )?;
    Ok(())
}

pub(crate) fn encode_operating_balance_metrics(
    w: &mut MetricsEncoder<Vec<u8>>,
    s: &State,
) -> std::io::Result<()> {
    let operating = s.operating_balance();
    let config = operating.config();
    for (name, value, help) in [
        (
            "operating_balance",
            operating.balance(),
            "Unbacked SOL credited by completed refill burns and by the fee surplus of SPL withdrawals, in lamports.",
        ),
        (
            "operating_balance_shortfall",
            operating.shortfall(),
            "Lamports that settlements spent above the operating balance. Later funding repays it first.",
        ),
        (
            "operating_refill_enabled",
            u64::from(config.enabled),
            "Whether new automatic operating refills are enabled.",
        ),
        (
            "operating_refill_low_water_mark",
            config.low_water_mark,
            "Operating SOL refill threshold in lamports.",
        ),
        (
            "operating_refill_target",
            config.target,
            "Operating SOL refill target in lamports.",
        ),
        (
            "operating_refill_pending",
            u64::from(operating.pending().is_some()),
            "Whether an operating refill is unresolved.",
        ),
        (
            "operating_refill_quarantined",
            u64::from(operating.pending().is_some_and(|p| p.quarantined)),
            "Whether the unresolved refill needs manual reconciliation.",
        ),
        (
            "operating_balance_low",
            u64::from(config.enabled && operating.balance() < config.low_water_mark),
            "Whether enabled operating funding is below its low-water mark.",
        ),
    ] {
        w.encode_gauge(name, value.metric_value(), help)?;
    }
    w.encode_counter(
        "operating_refills_completed",
        operating.completed_refills().metric_value(),
        "Completed refill burns, including deduplicated burns.",
    )?;
    w.encode_counter(
        "operating_refills_failed",
        operating.failed_refills().metric_value(),
        "Definitively rejected first refill attempts.",
    )?;
    storage::with_unstable_metrics(|m| -> std::io::Result<()> {
        // Absence after upgrade means no fresh read, not an empty top-up account.
        w.encode_gauge(
            "operating_top_up_balance_known",
            u64::from(m.operating_top_up_balance.is_some()).metric_value(),
            "Whether the top-up balance has been read since upgrade.",
        )?;
        if let Some(TopUpBalanceRead { balance, read_at }) = m.operating_top_up_balance {
            w.encode_gauge(
                "operating_top_up_balance",
                balance.metric_value(),
                "Last-read ckSOL top-up balance in lamports.",
            )?;
            w.encode_gauge(
                "operating_top_up_underfunded",
                u64::from(
                    config.enabled && balance < config.target,
                )
                .metric_value(),
                "Whether the last-read top-up account cannot fund a full refill from zero to target.",
            )?;
            w.encode_gauge(
                "operating_top_up_balance_read_timestamp_seconds",
                (read_at / NANOS_PER_SECOND).metric_value(),
                "Time of the last successful top-up balance read, in Unix seconds.",
            )?;
        }
        Ok(())
    })?;
    let mut errors = w.counter_vec(
        "operating_refill_errors",
        "Refill errors since upgrade, by reason.",
    )?;
    for reason in OperatingRefillErrorReason::ALL {
        errors = errors.value(
            &[("reason", reason.label())],
            storage::operating_refill_error_count(reason).metric_value(),
        )?;
    }
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
