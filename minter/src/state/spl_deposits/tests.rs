use super::*;
use crate::{
    address::{account_address, associated_token_address},
    state::{
        State,
        audit::{process_event, replay_events},
        event::{Event, EventType},
    },
    storage::{reset_events, with_event_iter},
    test_fixtures::{
        account, minter_public_key_fetched_event, runtime::TestCanisterRuntime, schnorr_master_key,
        valid_init_args,
    },
    utils::timed_sized_map::Timestamp,
};
use candid::Principal;
use cksol_types_internal::{
    SplTokenRegistration, SplTokenUpdate, UpgradeArgs, event::Event as CandidEvent,
};
use ic_stable_structures::Storable;
use std::{
    borrow::Cow,
    panic::{AssertUnwindSafe, catch_unwind},
};

fn deposit(owner: Account, mint: Address) -> QueuedSplDeposit {
    QueuedSplDeposit {
        account: owner,
        mint,
        address: associated_token_address(&account_address(&schnorr_master_key(), &owner), &mint),
        balance: 1_000,
        fee: 100,
    }
}

fn queued_event(deposit_id: DepositSplId, deposit: &QueuedSplDeposit) -> EventType {
    EventType::QueuedSplDeposit {
        deposit_id,
        account: deposit.account,
        mint: deposit.mint,
        address: deposit.address,
        balance: deposit.balance,
        fee: deposit.fee,
    }
}

#[test]
fn should_track_account_and_mint_pairs_independently() {
    let mut deposits = SplDeposits::default();
    assert_eq!(deposits.in_flight_id(&account(1), &[2; 32].into()), None);
    assert_eq!(deposits.latest_id(&account(1), &[2; 32].into()), None);
    assert_eq!(deposits.oldest_in_flight_queued_at(), None);
    let entries = [
        deposit(account(1), [2; 32].into()),
        deposit(account(1), [3; 32].into()),
        deposit(account(2), [2; 32].into()),
    ];
    for (id, entry) in entries.iter().enumerate() {
        let id = id as u64;
        deposits.queue(id, entry.clone(), 100 + id);
        assert_eq!(deposits.in_flight_id(&entry.account, &entry.mint), Some(id));
        assert_eq!(deposits.latest_id(&entry.account, &entry.mint), Some(id));
        assert_eq!(deposits.queued().get(&id), Some(entry));
    }
    assert_eq!(deposits.next_id(), 3);
    assert_eq!(deposits.oldest_in_flight_queued_at(), Some(100));
}

#[test]
fn should_reject_a_second_in_flight_deposit_for_the_same_pair() {
    let mut deposits = SplDeposits::default();
    let first = deposit(account(1), [2; 32].into());
    deposits.queue(0, first.clone(), 100);
    let before = deposits.clone();
    // None and Some([0; 32]) identify the same ICRC account and the same source ATA.
    let duplicate = deposit(
        Account {
            subaccount: Some([0; 32]),
            ..first.account
        },
        first.mint,
    );
    assert!(catch_unwind(AssertUnwindSafe(|| deposits.queue(1, duplicate, 200))).is_err());
    assert_eq!(deposits, before);
}

#[test]
fn should_reject_out_of_sequence_ids_and_invalid_net_amounts_before_mutation() {
    let valid = deposit(account(1), [2; 32].into());
    let cases = [
        (1, valid.clone()),
        (
            0,
            QueuedSplDeposit {
                balance: 0,
                fee: 0,
                ..valid.clone()
            },
        ),
        (
            0,
            QueuedSplDeposit {
                fee: valid.balance,
                ..valid.clone()
            },
        ),
        (
            0,
            QueuedSplDeposit {
                fee: valid.balance + 1,
                ..valid
            },
        ),
    ];
    for (id, entry) in cases {
        let mut deposits = SplDeposits::default();
        let before = deposits.clone();
        assert!(catch_unwind(AssertUnwindSafe(|| deposits.queue(id, entry, 100))).is_err());
        assert_eq!(deposits, before);
    }
}

#[test]
fn should_reject_id_overflow_before_recording_a_deposit() {
    let mut deposits = SplDeposits {
        next_id: u64::MAX,
        ..Default::default()
    };
    let before = deposits.clone();
    assert!(
        catch_unwind(AssertUnwindSafe(|| deposits.queue(
            u64::MAX,
            deposit(account(1), [2; 32].into()),
            100
        )))
        .is_err()
    );
    assert_eq!(deposits, before);
}

#[test]
fn should_keep_zero_fees_and_large_token_amounts_exact() {
    let mut deposits = SplDeposits::default();
    for (id, fee, net) in [(0, 0, u64::MAX), (1, u64::MAX - 1, 1)] {
        let entry = QueuedSplDeposit {
            balance: u64::MAX,
            fee,
            ..deposit(account(id as usize + 1), [2; 32].into())
        };
        deposits.queue(id, entry, 100);
        assert_eq!(deposits.queued()[&id].amount_to_mint(), net);
    }
}

#[test]
fn should_preserve_the_queued_event_tag_and_field_order() {
    let entry = deposit(account(1), [2; 32].into());
    let payload = queued_event(42, &entry);
    let encoded = minicbor::to_vec(&payload).unwrap();
    let mut decoder = minicbor::Decoder::new(&encoded);
    assert_eq!(decoder.array().unwrap(), Some(2));
    assert_eq!(decoder.u32().unwrap(), 16);
    assert_eq!(decoder.array().unwrap(), Some(6));
    assert_eq!(decoder.u64().unwrap(), 42);
    assert_eq!(decoder.decode::<Account>().unwrap(), entry.account);
    assert_eq!(decoder.bytes().unwrap(), entry.mint.as_ref());
    assert_eq!(decoder.bytes().unwrap(), entry.address.as_ref());
    assert_eq!(decoder.u64().unwrap(), entry.balance);
    assert_eq!(decoder.u64().unwrap(), entry.fee);
    assert_eq!(decoder.position(), encoded.len());
    assert_eq!(minicbor::decode::<EventType>(&encoded).unwrap(), payload);
}

#[test]
fn should_replay_captured_terms_without_changing_sol_or_operating_state() {
    reset_events();
    let args = valid_init_args();
    let mut state = State::try_from(args.clone()).unwrap();
    // IC time is fixed within a message: applying and recording an event see the same time.
    let runtime = TestCanisterRuntime::new().add_times((0..16u64).flat_map(|time| [time, time]));
    process_event(&mut state, minter_public_key_fetched_event(), &runtime);
    let mint = Address::from([2; 32]);
    process_event(
        &mut state,
        EventType::Upgrade(UpgradeArgs {
            add_spl_tokens: Some(vec![SplTokenRegistration {
                mint: mint.to_string(),
                token_program: "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA".into(),
                decimals: 6,
                ledger_id: Principal::from_slice(&[99, 1]),
                index_id: None,
                symbol: "ckTest".into(),
                minimum_deposit_amount: 1,
                deposit_fee: 100,
                deposits_paused: false,
            }]),
            ..Default::default()
        }),
        &runtime,
    );
    // Keep an existing SOL registration: an SPL queue event must not remove it.
    let owner = account(1);
    state
        .watchlist_mut()
        .register(
            Timestamp::from_nanos(0),
            owner,
            account_address(&schnorr_master_key(), &owner),
        )
        .unwrap();
    let snapshot = state.watchlist().snapshot();
    process_event(
        &mut state,
        EventType::WatchedDepositAddresses(snapshot),
        &runtime,
    );
    let mut without_spl_deposits = state.clone();
    let first = deposit(owner, mint);
    process_event(&mut state, queued_event(0, &first), &runtime);
    let update = UpgradeArgs {
        update_spl_tokens: Some(vec![SplTokenUpdate {
            mint: mint.to_string(),
            minimum_deposit_amount: None,
            deposit_fee: Some(200),
            deposits_paused: None,
        }]),
        ..Default::default()
    };
    process_event(&mut state, EventType::Upgrade(update.clone()), &runtime);
    without_spl_deposits.upgrade(update).unwrap();
    assert_eq!(state.spl_deposits().queued()[&0], first);
    assert_eq!(state.spl_deposits().queued()[&0].amount_to_mint(), 900);
    let second = QueuedSplDeposit {
        fee: 200,
        ..deposit(account(2), mint)
    };
    process_event(&mut state, queued_event(1, &second), &runtime);
    let events: Vec<Event> = with_event_iter(|events| {
        events
            .map(|event| {
                let decoded = Event::from_bytes(Cow::Owned(event.to_bytes().into_owned()));
                let wire = candid::encode_one(CandidEvent::from(event)).unwrap();
                let public: CandidEvent = candid::decode_one(&wire).unwrap();
                assert_eq!(Event::try_from(public).unwrap(), decoded);
                decoded
            })
            .collect()
    });
    let replayed = replay_events(
        std::iter::once(Event {
            timestamp: 0,
            payload: EventType::Init(args),
        })
        .chain(events),
    );
    assert_eq!(replayed, state);
    assert_eq!(replayed.spl_deposits().queued()[&1].amount_to_mint(), 800);
    assert!(runtime.sent_update_calls().is_empty());
    state.spl_deposits = SplDeposits::default();
    assert_eq!(state, without_spl_deposits);
}

#[test]
fn should_replay_a_sol_only_log_with_an_empty_spl_queue() {
    let state = replay_events([
        Event {
            timestamp: 0,
            payload: EventType::Init(valid_init_args()),
        },
        Event {
            timestamp: 1,
            payload: minter_public_key_fetched_event(),
        },
        Event {
            timestamp: 2,
            payload: EventType::QueuedDeposit {
                deposit_id: 0,
                account: account(1),
                address: account_address(&schnorr_master_key(), &account(1)),
                balance: crate::state::DepositBalance::new(10_000_000).unwrap(),
                deposit_fee: None,
                origin: crate::state::DepositOrigin::Manual,
            },
        },
    ]);
    assert_eq!(state.spl_deposits(), &SplDeposits::default());
    assert_eq!(state.deposits().next_id(), 1);
}

#[test]
fn should_restore_queue_timestamps_from_the_recorded_events() {
    let state = replay_events([
        Event {
            timestamp: 0,
            payload: EventType::Init(valid_init_args()),
        },
        Event {
            timestamp: 1,
            payload: minter_public_key_fetched_event(),
        },
        Event {
            timestamp: 42,
            payload: queued_event(0, &deposit(account(1), [2; 32].into())),
        },
        Event {
            timestamp: 80,
            payload: queued_event(1, &deposit(account(2), [2; 32].into())),
        },
    ]);
    assert_eq!(state.spl_deposits().queued_at_by_id[&0], 42);
    assert_eq!(state.spl_deposits().queued_at_by_id[&1], 80);
    assert_eq!(state.spl_deposits().oldest_in_flight_queued_at(), Some(42));
}

#[cfg(debug_assertions)]
#[test]
#[should_panic(expected = "unexpected source ATA")]
fn should_reject_a_queued_event_with_an_unexpected_source_ata_in_debug_builds() {
    let valid = deposit(account(1), [2; 32].into());
    let invalid = QueuedSplDeposit {
        // Use the ATA for a different mint, with the expected key already available.
        address: deposit(account(1), [3; 32].into()).address,
        ..valid
    };
    replay_events([
        Event {
            timestamp: 0,
            payload: EventType::Init(valid_init_args()),
        },
        Event {
            timestamp: 1,
            payload: minter_public_key_fetched_event(),
        },
        Event {
            timestamp: 42,
            payload: queued_event(0, &invalid),
        },
    ]);
}
