//! Mint rounds of credited deposits: selection, the ledger call and the classification of its reply.
use crate::{
    constants::{LEDGER_DEDUPLICATION_WINDOW, MAX_PENDING_MINTS_PER_ROUND},
    ledger::client::{LedgerClient, to_ledger_mint_index},
    numeric::LedgerMintIndex,
    queue_cursor::QueueCursor,
    runtime::CanisterRuntime,
    state::{State, audit::process_event, event::EventType, mutate_state, read_state},
    storage::{FailedMintReason, record_failed_mint_attempt},
};
use canlog::log;
use cksol_types_internal::log::Priority;
use ic_canister_runtime::Runtime;
use icrc_ledger_types::icrc1::transfer::{TransferArg, TransferError};
use scopeguard::ScopeGuard;
use std::{cell::RefCell, collections::BTreeMap, time::Duration};

/// The queues of pending mints that mint on a timer, each with its own rotation cursor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MintQueue {
    Sol,
}

thread_local! {
    static SOL_CURSOR: RefCell<QueueCursor<u64>> = const { RefCell::new(QueueCursor::new()) };
}

impl MintQueue {
    fn with_cursor<T>(self, f: impl FnOnce(&mut QueueCursor<u64>) -> T) -> T {
        let cursor = match self {
            Self::Sol => &SOL_CURSOR,
        };
        cursor.with(|cursor| f(&mut cursor.borrow_mut()))
    }

    fn record_failed_mint(self, reason: FailedMintReason) {
        match self {
            Self::Sol => record_failed_mint_attempt(reason),
        }
    }

    /// The unit of the minted amount, for logs.
    fn unit(self) -> &'static str {
        match self {
            Self::Sol => "lamports",
        }
    }
}

#[cfg(any(test, feature = "canbench-rs"))]
pub(crate) fn reset_cursors() {
    MintQueue::Sol.with_cursor(QueueCursor::clear);
}

/// Runs one bounded round, continuing after the previous round and wrapping at the end.
/// Returns whether the round made progress with more pending mints than one round holds.
pub(crate) async fn process_pending_mints_round<R: CanisterRuntime, Pending: Clone>(
    runtime: &R,
    queue: MintQueue,
    pending_mints: impl Fn(&State) -> &BTreeMap<u64, Pending>,
    mint: impl AsyncFn(&R, u64, Pending),
) -> bool {
    let (more_pending_mints_than_one_round, round): (bool, Vec<(u64, Pending)>) =
        read_state(|state| {
            let pending = pending_mints(state);
            (
                pending.len() > MAX_PENDING_MINTS_PER_ROUND,
                queue
                    .with_cursor(|cursor| cursor.round(pending, MAX_PENDING_MINTS_PER_ROUND))
                    .into_iter()
                    .map(|(id, pending)| (*id, pending.clone()))
                    .collect(),
            )
        });
    if round.is_empty() {
        return false;
    }
    let selected_ids: Vec<u64> = round.iter().map(|(id, _)| *id).collect();

    for (id, pending) in round {
        // Advance before the ledger call, including when its outcome remains retryable.
        queue.with_cursor(|cursor| cursor.advance(id));
        mint(runtime, id, pending).await;
    }

    let round_made_progress = read_state(|state| {
        let pending = pending_mints(state);
        selected_ids.iter().any(|id| !pending.contains_key(id))
    });
    if read_state(|state| pending_mints(state).is_empty()) {
        queue.with_cursor(QueueCursor::clear);
    }
    round_made_progress && more_pending_mints_than_one_round
}

/// Mints with fixed transfer arguments. A transient error is retried on the next round;
/// a definitive rejection or an unknown outcome quarantines the mint.
pub(crate) async fn mint_pending<R: CanisterRuntime, C: Runtime>(
    runtime: &R,
    queue: MintQueue,
    client: LedgerClient<C>,
    args: TransferArg,
    minted_event: impl Fn(LedgerMintIndex) -> EventType,
    quarantine_event: EventType,
    label: &str,
) {
    let record = |event: EventType| mutate_state(|state| process_event(state, event, runtime));
    let created_at_time = args
        .created_at_time
        .expect("BUG: a pending mint must have a timestamp");
    if is_beyond_deduplication_window(created_at_time, runtime.time()) {
        log!(
            Priority::Error,
            "Quarantining {label}: its pending mint is older than the deduplication window of the ledger"
        );
        queue.record_failed_mint(FailedMintReason::Expired);
        record(quarantine_event);
        return;
    }

    let quarantine_unless_defused = scopeguard::guard(quarantine_event, |event| {
        queue.record_failed_mint(FailedMintReason::UnknownOutcome);
        record(event);
    });

    let amount = args.amount.0.clone();
    match client.transfer(args).await {
        Ok(Ok(mint_block_index)) => {
            record(minted_event(mint_block_index));
            log!(
                Priority::Info,
                "Minted {amount} {} for {label} (ledger block index {})",
                queue.unit(),
                mint_block_index.get()
            );
            ScopeGuard::into_inner(quarantine_unless_defused);
        }
        Ok(Err(TransferError::Duplicate { duplicate_of })) => {
            let mint_block_index = to_ledger_mint_index(duplicate_of);
            record(minted_event(mint_block_index));
            log!(
                Priority::Info,
                "Mint of {label} was deduplicated by the ledger (ledger block index {})",
                mint_block_index.get()
            );
            ScopeGuard::into_inner(quarantine_unless_defused);
        }
        Ok(Err(TransferError::TooOld)) => {
            let quarantine_event = ScopeGuard::into_inner(quarantine_unless_defused);
            queue.record_failed_mint(FailedMintReason::Expired);
            log!(
                Priority::Error,
                "Quarantining {label}: the ledger rejected its mint as outside the deduplication window"
            );
            record(quarantine_event);
        }
        Ok(Err(
            rejection @ (TransferError::BadFee { .. }
            | TransferError::BadBurn { .. }
            | TransferError::InsufficientFunds { .. }),
        )) => {
            let quarantine_event = ScopeGuard::into_inner(quarantine_unless_defused);
            queue.record_failed_mint(FailedMintReason::Rejected);
            log!(
                Priority::Error,
                "Quarantining {label}: the ledger definitively rejected its mint from the minting account: {rejection:?}"
            );
            record(quarantine_event);
        }
        Ok(Err(
            transient
            @ (TransferError::TemporarilyUnavailable | TransferError::GenericError { .. }),
        )) => {
            ScopeGuard::into_inner(quarantine_unless_defused);
            queue.record_failed_mint(FailedMintReason::LedgerError);
            log!(
                Priority::Info,
                "Failed to mint {label}, retrying on the next round: {transient:?}"
            );
        }
        Ok(Err(clock_skew @ TransferError::CreatedInFuture { .. })) => {
            ScopeGuard::into_inner(quarantine_unless_defused);
            queue.record_failed_mint(FailedMintReason::CreatedInFuture);
            log!(
                Priority::Info,
                "Failed to mint {label}, retrying on the next round: {clock_skew:?}"
            );
        }
        Err(ic_error) => {
            ScopeGuard::into_inner(quarantine_unless_defused);
            queue.record_failed_mint(FailedMintReason::CallError);
            log!(
                Priority::Info,
                "Failed to mint {label}, retrying on the next round: {ic_error}"
            );
        }
    }
}

fn is_beyond_deduplication_window(created_at_time: u64, now: u64) -> bool {
    let age = Duration::from_nanos(now.saturating_sub(created_at_time));
    age > LEDGER_DEDUPLICATION_WINDOW
}
