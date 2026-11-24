//! Settlement rounds of observed sweeps: selection, fetching, retry classification and events.
use crate::{
    constants::MAX_CONCURRENT_RPC_CALLS,
    queue_cursor::QueueCursor,
    rpc::{FetchedTransaction, get_transaction},
    runtime::CanisterRuntime,
    state::{
        State, SweepSettlementError, audit::process_event, event::EventType, mutate_state,
        read_state,
    },
    storage::{FailedCreditReason, record_failed_credit_attempt},
};
use canlog::log;
use cksol_types_internal::log::Priority;
use solana_signature::Signature;
use std::{cell::RefCell, collections::BTreeMap};

/// The queues of observed sweeps that settle on a timer, each with its own rotation cursor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FinalizedQueue {
    Sol,
}

thread_local! {
    static SOL_CURSOR: RefCell<QueueCursor<Signature>> = const { RefCell::new(QueueCursor::new()) };
}

impl FinalizedQueue {
    fn with_cursor<T>(self, f: impl FnOnce(&mut QueueCursor<Signature>) -> T) -> T {
        let cursor = match self {
            Self::Sol => &SOL_CURSOR,
        };
        cursor.with(|cursor| f(&mut cursor.borrow_mut()))
    }

    fn record_failed_credit(self, reason: FailedCreditReason) {
        match self {
            Self::Sol => record_failed_credit_attempt(reason),
        }
    }
}

#[cfg(any(test, feature = "canbench-rs"))]
pub(crate) fn reset_cursors() {
    FinalizedQueue::Sol.with_cursor(QueueCursor::clear);
}

/// Settles matching sweep outcomes and quarantines mismatches. Unreadable outcomes stay queued.
/// Returns whether this round made progress and sweeps are left in the queue.
pub(crate) async fn credit_finalized_sweep_round<R: CanisterRuntime, S: Clone>(
    runtime: &R,
    queue: FinalizedQueue,
    finalized_sweeps: impl Fn(&State) -> &BTreeMap<Signature, S>,
    settle: impl Fn(Signature, S, &FetchedTransaction) -> Result<EventType, SweepSettlementError>,
    quarantine: impl Fn(Signature) -> EventType,
    label: &str,
) -> bool {
    let (finalized_count, round): (usize, Vec<(Signature, S)>) = read_state(|state| {
        let finalized = finalized_sweeps(state);
        (
            finalized.len(),
            queue
                .with_cursor(|cursor| cursor.round(finalized, MAX_CONCURRENT_RPC_CALLS))
                .into_iter()
                .map(|(signature, sweep)| (*signature, sweep.clone()))
                .collect(),
        )
    });
    let Some((last_signature, _)) = round.last() else {
        queue.with_cursor(QueueCursor::clear);
        return false;
    };
    // Continue past this round even if all its RPC reads fail.
    queue.with_cursor(|cursor| cursor.advance(*last_signature));

    let outcomes = futures::future::join_all(
        round
            .iter()
            .map(|(signature, _)| get_transaction(runtime, *signature)),
    )
    .await;

    let mut settled_count = 0;
    for ((signature, sweep), outcome) in round.into_iter().zip(outcomes) {
        let outcome = match outcome {
            Ok(Some(outcome)) => outcome,
            Ok(None) => {
                log!(
                    Priority::Error,
                    "Finalized {label} {signature} was not returned by getTransaction, retrying later"
                );
                queue.record_failed_credit(FailedCreditReason::NotFound);
                continue;
            }
            Err(e) if e.is_response_untrustworthy() => {
                log!(
                    Priority::Error,
                    "Failed to fetch finalized {label} {signature}: {e}, retrying later"
                );
                queue.record_failed_credit(FailedCreditReason::InvalidResponse);
                continue;
            }
            Err(e) => {
                log!(
                    Priority::Info,
                    "Failed to fetch finalized {label} {signature}: {e}, retrying later"
                );
                queue.record_failed_credit(FailedCreditReason::RpcError);
                continue;
            }
        };
        let event = match settle(signature, sweep, &outcome) {
            Ok(event) => event,
            Err(SweepSettlementError::Unreadable(e)) => {
                log!(
                    Priority::Error,
                    "Could not read the outcome of {label} {signature}: {e}, retrying later"
                );
                queue.record_failed_credit(FailedCreditReason::Unreadable);
                continue;
            }
            Err(SweepSettlementError::Mismatch(e)) => {
                log!(
                    Priority::Error,
                    "Quarantining the deposits of {label} {signature}: {e}"
                );
                queue.record_failed_credit(FailedCreditReason::Mismatch);
                quarantine(signature)
            }
        };
        mutate_state(|state| process_event(state, event, runtime));
        settled_count += 1;
    }

    if read_state(|state| finalized_sweeps(state).is_empty()) {
        queue.with_cursor(QueueCursor::clear);
    }
    settled_count > 0 && settled_count < finalized_count
}
