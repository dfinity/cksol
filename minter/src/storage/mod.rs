use crate::{
    runtime::CanisterRuntime,
    state::event::{Event, EventType},
};
use ic_stable_structures::{
    DefaultMemoryImpl, StableLog,
    memory_manager::{MemoryId, MemoryManager, VirtualMemory},
};
use std::cell::RefCell;

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
                  )
              )
        );
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

pub fn with_event_iter<F, R>(f: F) -> R
where
    F: for<'a> FnOnce(Box<dyn Iterator<Item = Event> + 'a>) -> R,
{
    EVENTS.with(|events| f(Box::new(events.borrow().iter())))
}

/// Purges unknown events from the event log by rewriting it with only known events.
pub fn purge_unknown_events() -> usize {
    use crate::state::event::EventType;
    let valid_events: Vec<Event> = with_event_iter(|iter| {
        iter.filter(|e| !matches!(e.payload, EventType::Unknown))
            .collect()
    });
    let original_count = total_event_count() as usize;
    let valid_count = valid_events.len();
    let purged = original_count - valid_count;
    if purged > 0 {
        MEMORY_MANAGER.with(|m| {
            EVENTS.with(|events| {
                *events.borrow_mut() = StableLog::new(
                    m.borrow().get(EVENT_LOG_INDEX_MEMORY_ID),
                    m.borrow().get(EVENT_LOG_DATA_MEMORY_ID),
                );
                for event in valid_events {
                    events
                        .borrow()
                        .append(&event)
                        .expect("re-recording event should succeed");
                }
            });
        });
    }
    purged
}

#[cfg(test)]
pub fn reset_events() {
    MEMORY_MANAGER.with(|m| {
        EVENTS.with(|events| {
            *events.borrow_mut() = StableLog::new(
                m.borrow().get(EVENT_LOG_INDEX_MEMORY_ID),
                m.borrow().get(EVENT_LOG_DATA_MEMORY_ID),
            );
        });
    });
}
