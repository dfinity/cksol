use super::*;
use crate::{
    state::{
        audit::replay_events,
        event::{Event, EventType},
    },
    test_fixtures::valid_init_args,
};
use cksol_types_internal::{UpgradeArgs, event::Event as CandidEvent};
use ic_stable_structures::Storable;
use std::borrow::Cow;

fn config() -> OperatingRefillConfig {
    OperatingRefillConfig {
        enabled: true,
        low_water_mark: 100,
        target: 1_000,
    }
}

#[test]
fn should_default_to_enabled_and_preserve_omitted_configuration() {
    let mut state = State::try_from(valid_init_args()).unwrap();
    assert_eq!(
        state.operating_balance().config(),
        OperatingRefillConfig {
            enabled: true,
            low_water_mark: 10_000_000,
            target: 100_000_000,
        }
    );
    assert_eq!(state.operating_balance().refill_amount(), 100_000_000);
    state
        .upgrade(UpgradeArgs {
            operating_balance_refill: Some(config()),
            ..Default::default()
        })
        .unwrap();
    state.upgrade(UpgradeArgs::default()).unwrap();
    assert_eq!(state.operating_balance().config(), config());
}

#[test]
fn should_reject_invalid_enabled_refill_configuration() {
    for (low_water_mark, target) in [(0, 100), (100, 100), (101, 100)] {
        let mut state = State::try_from(valid_init_args()).unwrap();
        assert_eq!(
            state.upgrade(UpgradeArgs {
                operating_balance_refill: Some(OperatingRefillConfig {
                    enabled: true,
                    low_water_mark,
                    target
                }),
                ..Default::default()
            }),
            Err(InvalidStateError::InvalidOperatingRefillConfig)
        );
    }
}

#[test]
fn should_credit_only_completed_burns_and_keep_ids_after_failure() {
    let mut funds = OperatingBalance::default();
    funds.set_config(config());
    assert_eq!(funds.refill_amount(), 1_000);
    funds.request_refill(0, 40, 10);
    assert_eq!(funds.balance(), 0);
    assert_eq!(funds.refill_amount(), 0);
    funds.fail_refill(0);
    assert_eq!(funds.next_refill_id(), 1);
    assert_eq!(funds.balance(), 0);
    funds.request_refill(1, 60, 20);
    funds.complete_refill(1);
    assert_eq!(funds.balance(), 60);
    assert_eq!(funds.refill_amount(), 940);
    assert_eq!(funds.completed_refills(), 1);
    assert_eq!(funds.failed_refills(), 1);
    funds.request_refill(2, 40, 30);
    funds.complete_refill(2);
    assert_eq!(funds.balance(), 100);
    assert_eq!(funds.refill_amount(), 0); // Exactly at the low-water mark.
}

#[test]
#[should_panic(expected = "no pending refill")]
fn should_not_credit_a_refill_twice() {
    let mut funds = OperatingBalance::default();
    funds.set_config(config());
    funds.request_refill(0, 100, 10);
    funds.complete_refill(0);
    funds.complete_refill(0);
}

#[test]
#[should_panic(expected = "unexpected refill ID")]
fn should_reject_an_outcome_for_a_different_refill() {
    let mut funds = OperatingBalance::default();
    funds.set_config(config());
    funds.request_refill(0, 100, 10);
    funds.complete_refill(1);
}

#[test]
fn should_replay_serialized_refill_events_with_disabled_or_changed_settings() {
    let events = vec![
        EventType::Init(valid_init_args()),
        EventType::Upgrade(UpgradeArgs {
            operating_balance_refill: Some(config()),
            ..Default::default()
        }),
        EventType::RequestedOperatingRefill {
            refill_id: 0,
            amount: 50,
            created_at_time: 100,
        },
        EventType::FailedOperatingRefill {
            refill_id: 0,
            reason: "InsufficientFunds".into(),
        },
        EventType::RequestedOperatingRefill {
            refill_id: 1,
            amount: 75,
            created_at_time: 200,
        },
        EventType::Upgrade(UpgradeArgs {
            operating_balance_refill: Some(OperatingRefillConfig {
                enabled: false,
                ..config()
            }),
            ..Default::default()
        }),
        EventType::CompletedOperatingRefill {
            refill_id: 1,
            burn_block_index: 42,
        },
        EventType::Upgrade(UpgradeArgs {
            operating_balance_refill: Some(config()),
            ..Default::default()
        }),
        EventType::RequestedOperatingRefill {
            refill_id: 2,
            amount: 80,
            created_at_time: 300,
        },
        EventType::QuarantinedOperatingRefill { refill_id: 2 },
    ];
    let events: Vec<_> = events
        .into_iter()
        .enumerate()
        .map(|(i, payload)| Event {
            timestamp: i as u64,
            payload,
        })
        .collect();
    // Every prefix is a valid replay checkpoint, including between intent and burn outcome.
    for end in 1..=events.len() {
        let prefix = &events[..end];
        let decoded: Vec<_> = prefix
            .iter()
            .map(|event| {
                let cbor = Event::from_bytes(Cow::Owned(event.to_bytes().into_owned()));
                let public = CandidEvent::from(Event {
                    timestamp: event.timestamp,
                    payload: event.payload.clone(),
                });
                assert_eq!(Event::try_from(public).unwrap(), cbor);
                cbor
            })
            .collect();
        assert_eq!(
            replay_events(prefix.iter().map(|e| Event {
                timestamp: e.timestamp,
                payload: e.payload.clone()
            })),
            replay_events(decoded)
        );
    }
    let state = replay_events(events);
    let funds = state.operating_balance();
    assert_eq!(state.balance(), 0); // Refill credits do not fabricate observed SOL.
    assert_eq!(funds.balance(), 75);
    assert_eq!(funds.next_refill_id(), 3);
    assert_eq!(
        funds.pending(),
        Some(PendingOperatingRefill {
            refill_id: 2,
            amount: 80,
            created_at_time: 300,
            quarantined: true
        })
    );
    assert_eq!(funds.refill_amount(), 0);
}

#[test]
fn should_keep_refill_event_tags_and_legacy_upgrade_fields() {
    let payloads = [
        (
            21,
            EventType::RequestedOperatingRefill {
                refill_id: 0,
                amount: 10,
                created_at_time: 42,
            },
        ),
        (
            22,
            EventType::CompletedOperatingRefill {
                refill_id: 0,
                burn_block_index: 1,
            },
        ),
        (
            23,
            EventType::FailedOperatingRefill {
                refill_id: 0,
                reason: "rejected".into(),
            },
        ),
        (24, EventType::QuarantinedOperatingRefill { refill_id: 0 }),
    ];
    for (tag, payload) in payloads {
        let encoded = minicbor::to_vec(&payload).unwrap();
        let mut decoder = minicbor::Decoder::new(&encoded);
        assert_eq!(decoder.array().unwrap(), Some(2));
        assert_eq!(decoder.u32().unwrap(), tag);
        assert_eq!(minicbor::decode::<EventType>(&encoded).unwrap(), payload);
    }
    #[derive(minicbor::Encode)]
    struct PreviousUpgradeArgs {
        #[n(8)]
        add_spl_tokens: Option<Vec<cksol_types_internal::SplTokenRegistration>>,
        #[n(9)]
        update_spl_tokens: Option<Vec<cksol_types_internal::SplTokenUpdate>>,
    }
    let bytes = minicbor::to_vec(PreviousUpgradeArgs {
        add_spl_tokens: None,
        update_spl_tokens: None,
    })
    .unwrap();
    let args: UpgradeArgs = minicbor::decode(&bytes).unwrap();
    assert_eq!(args.operating_balance_refill, None);
    let replayed = replay_events([
        Event {
            timestamp: 0,
            payload: EventType::Init(valid_init_args()),
        },
        Event {
            timestamp: 1,
            payload: EventType::Upgrade(args),
        },
    ]);
    assert!(replayed.operating_balance().config().enabled);
    assert_eq!(replayed.operating_balance().refill_amount(), 100_000_000);
}

#[test]
fn should_accept_any_thresholds_while_refills_are_disabled() {
    let mut state = State::try_from(valid_init_args()).unwrap();
    let disabled = OperatingRefillConfig {
        enabled: false,
        low_water_mark: 0,
        target: 0,
    };
    state
        .upgrade(UpgradeArgs {
            operating_balance_refill: Some(disabled),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(state.operating_balance().config(), disabled);
    assert_eq!(state.operating_balance().refill_amount(), 0);
}

#[test]
#[should_panic(expected = "refill is quarantined")]
fn should_reject_an_outcome_for_a_quarantined_refill() {
    let mut funds = OperatingBalance::default();
    funds.set_config(config());
    funds.request_refill(0, 100, 10);
    funds.quarantine_refill(0);
    funds.complete_refill(0);
}

#[test]
#[should_panic(expected = "unexpected refill ID")]
fn should_reject_a_request_with_an_unexpected_refill_id() {
    let mut funds = OperatingBalance::default();
    funds.set_config(config());
    funds.request_refill(1, 100, 10);
}

#[test]
#[should_panic(expected = "invalid refill amount")]
fn should_reject_a_request_for_a_zero_amount() {
    let mut funds = OperatingBalance::default();
    funds.set_config(config());
    funds.request_refill(0, 0, 10);
}

#[test]
#[should_panic(expected = "invalid refill amount")]
fn should_reject_a_request_above_the_refill_amount() {
    let mut funds = OperatingBalance::default();
    funds.set_config(config());
    funds.request_refill(0, 1_001, 10);
}

#[test]
fn should_repair_a_shortfall_from_every_source_of_operating_sol_before_it_is_spendable() {
    let mut operating = OperatingBalance::default();
    operating.credit(5);
    operating.reserve(5);

    // The balance is empty, so the complete cost becomes a shortfall.
    operating.debit(3);
    assert_eq!((operating.balance(), operating.shortfall()), (0, 3));

    // A returned reservation, a fee surplus and a refill each repay the shortfall first.
    operating.release(1);
    assert_eq!((operating.balance(), operating.shortfall()), (0, 2));
    operating.credit(1);
    assert_eq!((operating.balance(), operating.shortfall()), (0, 1));
    operating.request_refill(0, 10, 100);
    operating.complete_refill(0);
    assert_eq!((operating.balance(), operating.shortfall()), (9, 0));

    // A cost that the balance covers leaves no shortfall.
    operating.debit(4);
    assert_eq!((operating.balance(), operating.shortfall()), (5, 0));
    operating.debit(7);
    assert_eq!((operating.balance(), operating.shortfall()), (0, 2));
}
