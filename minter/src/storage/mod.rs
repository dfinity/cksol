use crate::{
    runtime::CanisterRuntime,
    state::event::{Event, EventType},
};
use ic_stable_structures::{
    DefaultMemoryImpl, StableLog,
    memory_manager::{MemoryId, MemoryManager, VirtualMemory},
};
use std::{cell::RefCell, collections::BTreeMap};

const EVENT_LOG_INDEX_MEMORY_ID: MemoryId = MemoryId::new(0);
const EVENT_LOG_DATA_MEMORY_ID: MemoryId = MemoryId::new(1);

type VMem = VirtualMemory<DefaultMemoryImpl>;
type EventLog = StableLog<Event, VMem, VMem>;

thread_local! {
    static MEMORY_MANAGER: RefCell<MemoryManager<DefaultMemoryImpl>> = RefCell::new(
        MemoryManager::init(DefaultMemoryImpl::default())
    );

    /// The log of the minter state modifications.
    static EVENTS: RefCell<EventLog> = MEMORY_MANAGER
        .with(|m|
              RefCell::new(
                  StableLog::init(
                      m.borrow().get(EVENT_LOG_INDEX_MEMORY_ID),
                      m.borrow().get(EVENT_LOG_DATA_MEMORY_ID)
                  ).expect("failed to initialize event log")
              )
        );

    static UNSTABLE_METRICS: RefCell<Metrics> = const { RefCell::new(Metrics::new()) };
}

#[derive(Default)]
pub(crate) struct Metrics {
    pub post_upgrade_instructions_consumed: u64,
    pub failed_credit_attempts: BTreeMap<FailedCreditReason, u64>,
    pub failed_mint_attempts: BTreeMap<FailedMintReason, u64>,
    pub withdrawal_transaction_rebroadcasts: u64,
    pub withdrawal_transactions_with_unresolved_outcome: u64,
}

impl Metrics {
    const fn new() -> Self {
        Self {
            post_upgrade_instructions_consumed: 0,
            failed_credit_attempts: BTreeMap::new(),
            failed_mint_attempts: BTreeMap::new(),
            withdrawal_transaction_rebroadcasts: 0,
            withdrawal_transactions_with_unresolved_outcome: 0,
        }
    }
}

/// Why an attempt to credit the deposits of a finalized sweep failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum FailedCreditReason {
    NotFound,
    RpcError,
    InvalidResponse,
    Unreadable,
    Mismatch,
}

impl FailedCreditReason {
    pub const ALL: [Self; 5] = [
        Self::NotFound,
        Self::RpcError,
        Self::InvalidResponse,
        Self::Unreadable,
        Self::Mismatch,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::NotFound => "not_found",
            Self::RpcError => "rpc_error",
            Self::InvalidResponse => "invalid_response",
            Self::Unreadable => "unreadable",
            Self::Mismatch => "mismatch",
        }
    }
}

pub(crate) fn record_failed_credit_attempt(reason: FailedCreditReason) {
    with_unstable_metrics_mut(|m| *m.failed_credit_attempts.entry(reason).or_insert(0) += 1);
}

pub(crate) fn failed_credit_attempt_count(reason: FailedCreditReason) -> u64 {
    with_unstable_metrics(|m| m.failed_credit_attempts.get(&reason).copied().unwrap_or(0))
}

/// Why an attempt to mint a pending deposit on the ckSOL ledger failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum FailedMintReason {
    Expired,
    Rejected,
    LedgerError,
    CreatedInFuture,
    CallError,
    UnknownOutcome,
}

impl FailedMintReason {
    pub const ALL: [Self; 6] = [
        Self::Expired,
        Self::Rejected,
        Self::LedgerError,
        Self::CreatedInFuture,
        Self::CallError,
        Self::UnknownOutcome,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Expired => "expired",
            Self::Rejected => "rejected",
            Self::LedgerError => "ledger_error",
            Self::CreatedInFuture => "created_in_future",
            Self::CallError => "call_error",
            Self::UnknownOutcome => "unknown_outcome",
        }
    }
}

pub(crate) fn record_failed_mint_attempt(reason: FailedMintReason) {
    with_unstable_metrics_mut(|m| *m.failed_mint_attempts.entry(reason).or_insert(0) += 1);
}

pub(crate) fn failed_mint_attempt_count(reason: FailedMintReason) -> u64 {
    with_unstable_metrics(|m| m.failed_mint_attempts.get(&reason).copied().unwrap_or(0))
}

/// Appends the event to the event log.
pub fn record_event<R: CanisterRuntime>(payload: EventType, runtime: &R) {
    EVENTS
        .with(|events| {
            events.borrow().append(&Event {
                timestamp: runtime.time(),
                payload,
            })
        })
        .expect("recording an event should succeed");
}

/// Returns the total number of events in the audit log.
pub fn total_event_count() -> u64 {
    EVENTS.with(|events| events.borrow().len())
}

pub(crate) fn with_unstable_metrics<F, R>(f: F) -> R
where
    F: FnOnce(&Metrics) -> R,
{
    UNSTABLE_METRICS.with(|m| f(&m.borrow()))
}

pub(crate) fn with_unstable_metrics_mut<F, R>(f: F) -> R
where
    F: FnOnce(&mut Metrics) -> R,
{
    UNSTABLE_METRICS.with(|m| f(&mut m.borrow_mut()))
}

pub fn with_event_iter<F, R>(f: F) -> R
where
    F: for<'a> FnOnce(Box<dyn Iterator<Item = Event> + 'a>) -> R,
{
    EVENTS.with(|events| f(Box::new(events.borrow().iter())))
}

#[cfg(any(test, feature = "canbench-rs"))]
pub(crate) fn reset_events() {
    MEMORY_MANAGER.with(|m| {
        EVENTS.with(|events| {
            *events.borrow_mut() = StableLog::new(
                m.borrow().get(EVENT_LOG_INDEX_MEMORY_ID),
                m.borrow().get(EVENT_LOG_DATA_MEMORY_ID),
            );
        });
    });
}
