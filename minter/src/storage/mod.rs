use crate::{
    runtime::CanisterRuntime,
    state::event::{DepositSource, Event, EventType, LegacyEvent, LegacyEventType},
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
type LegacyEventLog = StableLog<LegacyEvent, VMem, VMem>;

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

/// One-time migration: reads all stored events using `LegacyEvent` (which can decode
/// the old `AcceptedManualDeposit` format at CBOR index 2), converts them to the new
/// `Event` encoding, and rebuilds the stable log.
///
/// Safe to call multiple times: events already in the new format decode correctly via
/// `LegacyEventType` because all non-`AcceptedManualDeposit` variants are identical.
///
/// Remove after the migration has been confirmed on the deployed canister.
pub fn migrate_event_log() {
    let migrated: Vec<Event> = MEMORY_MANAGER.with(|m| {
        let legacy_log: LegacyEventLog = StableLog::init(
            m.borrow().get(EVENT_LOG_INDEX_MEMORY_ID),
            m.borrow().get(EVENT_LOG_DATA_MEMORY_ID),
        )
        .expect("failed to init legacy event log for migration");
        legacy_log.iter().map(legacy_event_to_event).collect()
    });

    MEMORY_MANAGER.with(|m| {
        EVENTS.with(|events| {
            let new_log = StableLog::new(
                m.borrow().get(EVENT_LOG_INDEX_MEMORY_ID),
                m.borrow().get(EVENT_LOG_DATA_MEMORY_ID),
            );
            for event in migrated {
                new_log
                    .append(&event)
                    .expect("event migration should succeed");
            }
            *events.borrow_mut() = new_log;
        });
    });
}

fn legacy_event_to_event(legacy: LegacyEvent) -> Event {
    Event {
        timestamp: legacy.timestamp,
        payload: match legacy.payload {
            LegacyEventType::Init(args) => EventType::Init(args),
            LegacyEventType::Upgrade(args) => EventType::Upgrade(args),
            LegacyEventType::AcceptedManualDeposit {
                deposit_id,
                deposit_amount,
                amount_to_mint,
            } => EventType::AcceptedDeposit {
                deposit_id,
                deposit_amount,
                amount_to_mint,
                source: DepositSource::Manual,
            },
            LegacyEventType::QuarantinedDeposit(id) => EventType::QuarantinedDeposit(id),
            LegacyEventType::Minted {
                deposit_id,
                mint_block_index,
            } => EventType::Minted {
                deposit_id,
                mint_block_index,
            },
            LegacyEventType::AcceptedWithdrawalRequest(r) => {
                EventType::AcceptedWithdrawalRequest(r)
            }
            LegacyEventType::SubmittedTransaction {
                signature,
                message,
                signers,
                slot,
                purpose,
            } => EventType::SubmittedTransaction {
                signature,
                message,
                signers,
                slot,
                purpose,
            },
            LegacyEventType::ResubmittedTransaction {
                old_signature,
                new_signature,
                new_slot,
            } => EventType::ResubmittedTransaction {
                old_signature,
                new_signature,
                new_slot,
            },
            LegacyEventType::SucceededTransaction { signature } => {
                EventType::SucceededTransaction { signature }
            }
            LegacyEventType::FailedTransaction { signature } => {
                EventType::FailedTransaction { signature }
            }
            LegacyEventType::ExpiredTransaction { signature } => {
                EventType::ExpiredTransaction { signature }
            }
            LegacyEventType::StartedMonitoringAccount { account } => {
                EventType::StartedMonitoringAccount { account }
            }
            LegacyEventType::StoppedMonitoringAccount { account } => {
                EventType::StoppedMonitoringAccount { account }
            }
        },
    }
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

/// Writes legacy-format events to the stable log for testing the migration.
#[cfg(test)]
pub(crate) fn write_legacy_events_for_test(events: Vec<LegacyEvent>) {
    MEMORY_MANAGER.with(|m| {
        let log: LegacyEventLog = StableLog::new(
            m.borrow().get(EVENT_LOG_INDEX_MEMORY_ID),
            m.borrow().get(EVENT_LOG_DATA_MEMORY_ID),
        );
        for event in events {
            log.append(&event)
                .expect("writing legacy event should succeed");
        }
        EVENTS.with(|ev| {
            *ev.borrow_mut() = StableLog::init(
                m.borrow().get(EVENT_LOG_INDEX_MEMORY_ID),
                m.borrow().get(EVENT_LOG_DATA_MEMORY_ID),
            )
            .expect("failed to re-init EVENTS after writing legacy events");
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::event::{DepositId, DepositSource, LegacyEvent, LegacyEventType};
    use candid::Principal;
    use icrc_ledger_types::icrc1::account::Account;
    use solana_signature::Signature;

    #[test]
    fn migrate_event_log_converts_accepted_manual_deposit() {
        reset_events();

        let deposit_id = DepositId {
            signature: Signature::default(),
            account: Account {
                owner: Principal::anonymous(),
                subaccount: None,
            },
        };

        write_legacy_events_for_test(vec![
            LegacyEvent {
                timestamp: 100,
                payload: LegacyEventType::AcceptedManualDeposit {
                    deposit_id,
                    deposit_amount: 2_000_000,
                    amount_to_mint: 1_800_000,
                },
            },
            LegacyEvent {
                timestamp: 200,
                payload: LegacyEventType::QuarantinedDeposit(DepositId {
                    signature: Signature::default(),
                    account: Account {
                        owner: Principal::anonymous(),
                        subaccount: None,
                    },
                }),
            },
        ]);

        migrate_event_log();

        let events: Vec<Event> = with_event_iter(|iter| iter.collect());

        assert_eq!(events.len(), 2);

        assert_eq!(events[0].timestamp, 100);
        assert!(
            matches!(
                &events[0].payload,
                EventType::AcceptedDeposit {
                    deposit_amount,
                    amount_to_mint,
                    source: DepositSource::Manual,
                    ..
                }
                if *deposit_amount == 2_000_000 && *amount_to_mint == 1_800_000
            ),
            "expected AcceptedDeposit with source=Manual, got {:?}",
            events[0].payload
        );

        assert_eq!(events[1].timestamp, 200);
        assert!(matches!(
            &events[1].payload,
            EventType::QuarantinedDeposit(_)
        ));
    }

    #[test]
    fn migrate_event_log_is_idempotent() {
        reset_events();

        let deposit_id = DepositId {
            signature: Signature::default(),
            account: Account {
                owner: Principal::anonymous(),
                subaccount: None,
            },
        };

        write_legacy_events_for_test(vec![LegacyEvent {
            timestamp: 42,
            payload: LegacyEventType::AcceptedManualDeposit {
                deposit_id,
                deposit_amount: 1_000_000,
                amount_to_mint: 900_000,
            },
        }]);

        // Run migration twice — second run should be a no-op
        migrate_event_log();
        migrate_event_log();

        let events: Vec<Event> = with_event_iter(|iter| iter.collect());
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0].payload,
            EventType::AcceptedDeposit {
                source: DepositSource::Manual,
                ..
            }
        ));
    }
}
