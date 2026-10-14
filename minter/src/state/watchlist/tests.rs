use crate::{
    state::{
        QueuedDeposit,
        event::{WatchedDepositAddress, WatchlistSnapshot},
        watchlist::{
            MAX_WATCHED_ADDRESSES, RearmError, WatchedAddress, Watchlist, WatchlistFull, scan_slots,
        },
    },
    test_fixtures::{
        AUTOMATIC_DEPOSIT_FEE, HOUR_NANOS, MINIMUM_DEPOSIT_AMOUNT, MINUTE_NANOS, REGISTERED_AT,
        WATCH_WINDOW_NANOS, account, at, automatic_deposit_of, deposit_address, register,
    },
};
use icrc_ledger_types::icrc1::account::Account;

const VALID_UNTIL: u64 = REGISTERED_AT + WATCH_WINDOW_NANOS;
const LAST_SLOT_AT: u64 = VALID_UNTIL - 30 * MINUTE_NANOS;

#[test]
fn should_register_an_account_for_the_window() {
    let mut watchlist = Watchlist::default();

    let watched = register(&mut watchlist, at(REGISTERED_AT), account(1));

    assert_eq!(watched, unscanned(account(1)));
    assert_eq!(watchlist.get(at(REGISTERED_AT), &account(1)), Some(watched));
    assert_eq!(watchlist.get(at(VALID_UNTIL), &account(1)), Some(watched));
    assert_eq!(watchlist.get(at(VALID_UNTIL + 1), &account(1)), None);
    assert_eq!(watchlist.len(), 1);
}

#[test]
fn should_keep_the_stored_registration_when_registering_again() {
    let mut watchlist = Watchlist::default();
    let first = register(&mut watchlist, at(REGISTERED_AT), account(1));
    watchlist.record_scan(at(REGISTERED_AT + MINUTE_NANOS), &account(1));
    let scanned = watchlist
        .get(at(REGISTERED_AT + MINUTE_NANOS), &account(1))
        .unwrap();

    let second = register(&mut watchlist, at(REGISTERED_AT + HOUR_NANOS), account(1));

    assert_eq!(second, scanned);
    assert_eq!(second.valid_until, first.valid_until);
    assert_eq!(watchlist.len(), 1);
}

#[test]
fn should_reject_a_registration_when_full_of_live_entries() {
    let mut watchlist = Watchlist::default();
    for index in 0..MAX_WATCHED_ADDRESSES.get() {
        register(&mut watchlist, at(REGISTERED_AT), account(index));
    }
    let extra = account(MAX_WATCHED_ADDRESSES.get());

    let result = watchlist.register(at(VALID_UNTIL), extra, deposit_address(extra));

    assert_eq!(result, Err(WatchlistFull));
    assert_eq!(watchlist.len(), MAX_WATCHED_ADDRESSES.get());
}

#[test]
fn should_be_due_for_the_first_scan_until_scanned() {
    let mut watchlist = Watchlist::default();
    register(&mut watchlist, at(REGISTERED_AT), account(1));

    assert_eq!(
        watchlist.due(at(REGISTERED_AT)),
        vec![unscanned(account(1))]
    );
    assert_eq!(
        watchlist.due(at(REGISTERED_AT + HOUR_NANOS)),
        vec![unscanned(account(1))]
    );
}

#[test]
fn should_follow_the_scan_slots_and_stop_after_the_last_one() {
    let mut watchlist = Watchlist::default();
    register(&mut watchlist, at(REGISTERED_AT), account(1));
    let slots_at: Vec<u64> = scan_slots()
        .map(|slot| REGISTERED_AT + slot.as_nanos() as u64)
        .collect();
    assert_eq!(slots_at.len(), 33);
    assert_eq!(slots_at[9], REGISTERED_AT + 30 * MINUTE_NANOS);
    assert_eq!(slots_at[32], LAST_SLOT_AT);

    for (index, &slot_at) in slots_at.iter().enumerate() {
        let watched = watchlist.get(at(slot_at), &account(1)).unwrap();
        assert_eq!(watched.scan_count, index as u32);
        assert_eq!(watched.next_scan_at, Some(at(slot_at)));
        assert_eq!(watchlist.due(at(slot_at - 1)), vec![]);
        assert_eq!(watchlist.due(at(slot_at)), vec![watched]);

        watchlist.record_scan(at(slot_at), &account(1));
    }

    let watched = watchlist.get(at(LAST_SLOT_AT), &account(1)).unwrap();
    assert_eq!(watched.scan_count, 33);
    assert_eq!(watched.last_scanned_at, Some(at(LAST_SLOT_AT)));
    assert_eq!(watched.next_scan_at, None);
    assert_eq!(watchlist.due(at(VALID_UNTIL)), vec![]);
}

#[test]
fn should_skip_the_slots_passed_before_a_late_scan() {
    let mut watchlist = Watchlist::default();
    register(&mut watchlist, at(REGISTERED_AT), account(1));

    watchlist.record_scan(at(REGISTERED_AT + 10 * MINUTE_NANOS), &account(1));
    let watched = watchlist
        .get(at(REGISTERED_AT + 10 * MINUTE_NANOS), &account(1))
        .unwrap();
    assert_eq!(watched.scan_count, 1);
    assert_eq!(
        watched.next_scan_at,
        Some(at(REGISTERED_AT + 15 * MINUTE_NANOS))
    );

    watchlist.record_scan(at(REGISTERED_AT + 15 * MINUTE_NANOS + 1), &account(1));
    let watched = watchlist
        .get(at(REGISTERED_AT + 15 * MINUTE_NANOS + 1), &account(1))
        .unwrap();
    assert_eq!(watched.scan_count, 2);
    assert_eq!(
        watched.next_scan_at,
        Some(at(REGISTERED_AT + 20 * MINUTE_NANOS))
    );
}

#[test]
fn should_report_no_next_scan_after_the_last_slot() {
    let mut watchlist = Watchlist::default();
    register(&mut watchlist, at(REGISTERED_AT), account(1));

    watchlist.record_scan(at(LAST_SLOT_AT - 1), &account(1));
    assert_eq!(
        watchlist
            .get(at(VALID_UNTIL), &account(1))
            .unwrap()
            .next_scan_at,
        Some(at(LAST_SLOT_AT))
    );

    watchlist.record_scan(at(LAST_SLOT_AT), &account(1));
    assert_eq!(
        watchlist
            .get(at(VALID_UNTIL), &account(1))
            .unwrap()
            .next_scan_at,
        None
    );
}

#[test]
fn should_ignore_a_scan_of_an_account_that_is_not_watched() {
    let mut watchlist = Watchlist::default();
    register(&mut watchlist, at(REGISTERED_AT), account(1));

    watchlist.record_scan(at(REGISTERED_AT), &account(2));
    watchlist.record_scan(at(VALID_UNTIL + 1), &account(1));

    assert_eq!(
        watchlist.iter().collect::<Vec<_>>(),
        vec![unscanned(account(1))]
    );
}

#[test]
fn should_count_the_live_registrations_and_report_the_oldest() {
    let mut watchlist = Watchlist::default();
    register(&mut watchlist, at(REGISTERED_AT), account(1));
    register(&mut watchlist, at(REGISTERED_AT + HOUR_NANOS), account(2));

    assert_eq!(watchlist.live_len(at(VALID_UNTIL)), 2);
    assert_eq!(
        watchlist.oldest_registered_at(at(VALID_UNTIL)),
        Some(at(REGISTERED_AT))
    );
    assert_eq!(watchlist.live_len(at(VALID_UNTIL + 1)), 1);
    assert_eq!(
        watchlist.oldest_registered_at(at(VALID_UNTIL + 1)),
        Some(at(REGISTERED_AT + HOUR_NANOS))
    );
    assert_eq!(watchlist.live_len(at(VALID_UNTIL + HOUR_NANOS + 1)), 0);
    assert_eq!(
        watchlist.oldest_registered_at(at(VALID_UNTIL + HOUR_NANOS + 1)),
        None
    );
    assert_eq!(watchlist.len(), 2);
}

#[test]
fn should_rearm_an_account_for_a_fresh_window_from_the_first_slot() {
    let mut watchlist = Watchlist::default();
    let rearmed_at = REGISTERED_AT + 2 * HOUR_NANOS;

    let rearmed = watchlist
        .rearm(at(rearmed_at), &dropped(account(1)))
        .unwrap();

    assert_eq!(
        rearmed,
        WatchedAddress {
            valid_until: at(rearmed_at + WATCH_WINDOW_NANOS),
            next_scan_at: Some(at(rearmed_at)),
            ..unscanned(account(1))
        }
    );
    assert_eq!(watchlist.due(at(rearmed_at)), vec![rearmed]);
    watchlist.record_scan(at(rearmed_at), &account(1));
    assert_eq!(
        watchlist
            .get(at(rearmed_at), &account(1))
            .unwrap()
            .next_scan_at,
        Some(at(rearmed_at + MINUTE_NANOS))
    );
    assert_eq!(
        watchlist.get(at(rearmed_at + WATCH_WINDOW_NANOS + 1), &account(1)),
        None
    );
}

#[test]
fn should_not_rearm_an_account_when_full_of_live_entries() {
    let mut watchlist = Watchlist::default();
    for index in 1..=MAX_WATCHED_ADDRESSES.get() {
        register(&mut watchlist, at(REGISTERED_AT), account(index));
    }

    let result = watchlist.rearm(at(REGISTERED_AT + HOUR_NANOS), &dropped(account(0)));

    assert_eq!(result, Err(RearmError::WatchlistFull));
    assert_eq!(
        watchlist.get(at(REGISTERED_AT + HOUR_NANOS), &account(0)),
        None
    );
}

#[test]
fn should_not_rearm_a_manual_deposit() {
    let mut watchlist = Watchlist::default();
    let manual = QueuedDeposit {
        deposit_fee: None,
        ..dropped(account(1))
    };

    let result = watchlist.rearm(at(REGISTERED_AT), &manual);

    assert_eq!(result, Err(RearmError::ManualDeposit));
    assert!(watchlist.is_empty());
}

#[test]
fn should_keep_the_registration_of_an_account_that_is_already_watched() {
    let mut watchlist = Watchlist::default();
    let registered = register(&mut watchlist, at(REGISTERED_AT + HOUR_NANOS), account(1));

    let rearmed = watchlist
        .rearm(at(REGISTERED_AT + 2 * HOUR_NANOS), &dropped(account(1)))
        .unwrap();

    assert_eq!(rearmed, registered);
    assert_eq!(watchlist.len(), 1);
}

#[test]
fn should_rebuild_the_watchlist_verbatim_from_its_snapshot() {
    let mut watchlist = Watchlist::default();
    register(&mut watchlist, at(REGISTERED_AT + HOUR_NANOS), account(1));
    register(&mut watchlist, at(REGISTERED_AT), account(3));
    register(&mut watchlist, at(REGISTERED_AT), account(2));
    watchlist.record_scan(at(REGISTERED_AT + MINUTE_NANOS), &account(2));
    let snapshot = watchlist.snapshot();

    let rebuilt = Watchlist::from_snapshot(&snapshot);

    assert_eq!(rebuilt, watchlist);
    assert_eq!(
        snapshot
            .entries
            .iter()
            .map(|entry| entry.account)
            .collect::<Vec<_>>(),
        vec![account(3), account(2), account(1)]
    );
    assert_eq!(snapshot.window_nanos, WATCH_WINDOW_NANOS);
    assert_eq!(snapshot.capacity, MAX_WATCHED_ADDRESSES.get() as u64);
}

#[test]
fn should_keep_expired_registrations_and_exceed_the_capacity_when_rebuilding() {
    let entries: Vec<WatchedDepositAddress> = (0..=MAX_WATCHED_ADDRESSES.get())
        .map(|index| WatchedDepositAddress {
            account: account(index),
            address: deposit_address(account(index)),
            valid_until: VALID_UNTIL,
            scheduled_from: REGISTERED_AT,
            scan_count: 3,
            last_scanned_at: Some(REGISTERED_AT + HOUR_NANOS),
        })
        .collect();
    let snapshot = WatchlistSnapshot {
        window_nanos: WATCH_WINDOW_NANOS,
        capacity: MAX_WATCHED_ADDRESSES.get() as u64,
        entries: entries.clone(),
    };

    let rebuilt = Watchlist::from_snapshot(&snapshot);

    assert_eq!(rebuilt.len(), entries.len());
    assert_eq!(rebuilt.snapshot(), snapshot);
    assert_eq!(rebuilt.live_len(at(VALID_UNTIL + 1)), 0);
    assert_eq!(
        rebuilt.get(at(VALID_UNTIL), &account(0)),
        Some(WatchedAddress {
            account: account(0),
            address: deposit_address(account(0)),
            valid_until: at(VALID_UNTIL),
            scan_count: 3,
            last_scanned_at: Some(at(REGISTERED_AT + HOUR_NANOS)),
            next_scan_at: Some(at(REGISTERED_AT + 90 * MINUTE_NANOS)),
        })
    );
}

#[test]
#[should_panic(expected = "non-zero capacity")]
fn should_panic_when_rebuilding_from_a_snapshot_without_capacity() {
    Watchlist::from_snapshot(&WatchlistSnapshot {
        window_nanos: WATCH_WINDOW_NANOS,
        capacity: 0,
        entries: vec![],
    });
}

fn dropped(account: Account) -> QueuedDeposit {
    automatic_deposit_of(account, MINIMUM_DEPOSIT_AMOUNT, AUTOMATIC_DEPOSIT_FEE)
}

fn unscanned(account: Account) -> WatchedAddress {
    WatchedAddress {
        account,
        address: deposit_address(account),
        valid_until: at(VALID_UNTIL),
        scan_count: 0,
        last_scanned_at: None,
        next_scan_at: Some(at(REGISTERED_AT)),
    }
}
