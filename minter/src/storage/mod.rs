use crate::{
    runtime::CanisterRuntime,
    state::event::{Event, EventType},
};
use ic_stable_structures::{
    DefaultMemoryImpl, StableBTreeMap, StableLog,
    memory_manager::{MemoryId, MemoryManager, VirtualMemory},
};
use icrc_ledger_types::icrc1::account::Account;
use std::cell::RefCell;

const EVENT_LOG_INDEX_MEMORY_ID: MemoryId = MemoryId::new(0);
const EVENT_LOG_DATA_MEMORY_ID: MemoryId = MemoryId::new(1);
const DISCOVERED_SIGNATURES_MEMORY_ID: MemoryId = MemoryId::new(2);

type VMem = VirtualMemory<DefaultMemoryImpl>;
type EventLog = StableLog<Event, VMem, VMem>;
/// Maps signature bytes to the account that owns the deposit.
/// Using signature as key since Solana transaction signatures are globally unique.
type DiscoveredSignatures = StableBTreeMap<[u8; 64], Account, VMem>;

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

    /// Queue of discovered deposit transaction signatures awaiting processing.
    static DISCOVERED_SIGNATURES: RefCell<DiscoveredSignatures> = MEMORY_MANAGER
        .with(|m| RefCell::new(StableBTreeMap::init(m.borrow().get(DISCOVERED_SIGNATURES_MEMORY_ID))));

    static UNSTABLE_METRICS: RefCell<Metrics> = const { RefCell::new(Metrics::new()) };
}

#[derive(Default)]
pub(crate) struct Metrics {
    pub post_upgrade_instructions_consumed: u64,
}

impl Metrics {
    const fn new() -> Self {
        Self {
            post_upgrade_instructions_consumed: 0,
        }
    }
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

pub fn with_discovered_signatures_mut<F, R>(f: F) -> R
where
    F: FnOnce(&mut DiscoveredSignatures) -> R,
{
    DISCOVERED_SIGNATURES.with(|q| f(&mut q.borrow_mut()))
}

pub fn with_discovered_signatures<F, R>(f: F) -> R
where
    F: FnOnce(&DiscoveredSignatures) -> R,
{
    DISCOVERED_SIGNATURES.with(|q| f(&q.borrow()))
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

#[cfg(any(test, feature = "canbench-rs"))]
pub(crate) fn reset_discovered_signatures() {
    MEMORY_MANAGER.with(|m| {
        DISCOVERED_SIGNATURES.with(|q| {
            *q.borrow_mut() = StableBTreeMap::new(m.borrow().get(DISCOVERED_SIGNATURES_MEMORY_ID));
        });
    });
}
