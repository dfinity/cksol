use crate::{
    state::watchlist::{
        MAX_WATCHED_ADDRESSES, SCAN_GAPS_SECS, WatchedAddress, Watchlist, WatchlistFull,
    },
    test_fixtures::{
        HOUR_NANOS, MINUTE_NANOS, REGISTERED_AT, WATCH_WINDOW_NANOS, account, deposit_address,
        register,
    },
};
use icrc_ledger_types::icrc1::account::Account;
use std::time::Duration;

const VALID_UNTIL: u64 = REGISTERED_AT + WATCH_WINDOW_NANOS;

#[test]
fn should_register_an_account_for_the_window() {
    let mut watchlist = Watchlist::default();

    let watched = register(&mut watchlist, REGISTERED_AT, account(1));

    assert_eq!(watched, unscanned(account(1)));
    assert_eq!(watchlist.get(REGISTERED_AT, &account(1)), Some(watched));
    assert_eq!(watchlist.get(VALID_UNTIL, &account(1)), Some(watched));
    assert_eq!(watchlist.get(VALID_UNTIL + 1, &account(1)), None);
    assert_eq!(watchlist.len(), 1);
}

#[test]
fn should_keep_the_stored_registration_when_registering_again() {
    let mut watchlist = Watchlist::default();
    let first = register(&mut watchlist, REGISTERED_AT, account(1));
    watchlist.record_scan(REGISTERED_AT + MINUTE_NANOS, &account(1));
    let scanned = watchlist
        .get(REGISTERED_AT + MINUTE_NANOS, &account(1))
        .unwrap();

    let second = register(&mut watchlist, REGISTERED_AT + HOUR_NANOS, account(1));

    assert_eq!(second, scanned);
    assert_eq!(second.valid_until, first.valid_until);
    assert_eq!(watchlist.len(), 1);
}

#[test]
fn should_register_anew_once_the_window_has_passed() {
    let mut watchlist = Watchlist::default();
    register(&mut watchlist, REGISTERED_AT, account(1));
    watchlist.record_scan(REGISTERED_AT + MINUTE_NANOS, &account(1));

    let renewed = register(&mut watchlist, VALID_UNTIL + 1, account(1));

    assert_eq!(
        renewed,
        WatchedAddress {
            valid_until: VALID_UNTIL + 1 + WATCH_WINDOW_NANOS,
            next_scan_at: Some(VALID_UNTIL + 1),
            ..unscanned(account(1))
        }
    );
    assert_eq!(watchlist.len(), 1);
}

#[test]
fn should_reject_a_registration_when_full_of_live_entries() {
    let mut watchlist = full_watchlist();
    let extra = account(MAX_WATCHED_ADDRESSES.get());

    let result = watchlist.register(VALID_UNTIL, extra, deposit_address(extra));

    assert_eq!(result, Err(WatchlistFull));
    assert_eq!(watchlist.len(), MAX_WATCHED_ADDRESSES.get());
}

#[test]
fn should_keep_an_existing_registration_while_full() {
    let mut watchlist = full_watchlist();

    let result = watchlist.register(
        REGISTERED_AT + HOUR_NANOS,
        account(0),
        deposit_address(account(0)),
    );

    assert_eq!(result, Ok(unscanned(account(0))));
    assert_eq!(watchlist.len(), MAX_WATCHED_ADDRESSES.get());
}

#[test]
fn should_admit_a_registration_once_a_window_of_a_full_watchlist_has_passed() {
    let mut watchlist = full_watchlist();
    let extra = account(MAX_WATCHED_ADDRESSES.get());

    let result = watchlist.register(VALID_UNTIL + 1, extra, deposit_address(extra));

    assert!(result.is_ok());
    assert_eq!(watchlist.len(), 1);
}

#[test]
fn should_be_due_for_the_first_scan_right_after_registration() {
    let mut watchlist = Watchlist::default();
    register(&mut watchlist, REGISTERED_AT, account(1));

    assert_eq!(watchlist.due(REGISTERED_AT), vec![unscanned(account(1))]);
    assert_eq!(watchlist.due(REGISTERED_AT - 1), vec![]);
}

#[test]
fn should_follow_the_scan_schedule_and_stop_before_the_window_closes() {
    let mut watchlist = Watchlist::default();
    register(&mut watchlist, REGISTERED_AT, account(1));
    let mut scanned_at = REGISTERED_AT;

    for (scan_count, gap_secs) in SCAN_GAPS_SECS.into_iter().enumerate() {
        watchlist.record_scan(scanned_at, &account(1));
        let watched = watchlist.get(scanned_at, &account(1)).unwrap();
        let next_scan_at = scanned_at + gap_secs * 1_000_000_000;

        assert_eq!(watched.scan_count, scan_count as u32 + 1);
        assert_eq!(watched.last_scanned_at, Some(scanned_at));
        assert_eq!(watched.next_scan_at, Some(next_scan_at));
        assert_eq!(watchlist.due(next_scan_at - 1), vec![]);
        assert_eq!(watchlist.due(next_scan_at), vec![watched]);
        scanned_at = next_scan_at;
    }
    assert_eq!(
        scanned_at,
        REGISTERED_AT + Duration::from_mins(30 + 23 * 60).as_nanos() as u64
    );

    watchlist.record_scan(scanned_at, &account(1));
    let watched = watchlist.get(scanned_at, &account(1)).unwrap();

    assert_eq!(watched.scan_count, 33);
    assert_eq!(watched.next_scan_at, None);
    assert_eq!(watchlist.due(VALID_UNTIL), vec![]);
}

#[test]
fn should_report_no_next_scan_when_the_gap_ends_after_the_window() {
    let mut watchlist = Watchlist::default();
    register(&mut watchlist, REGISTERED_AT, account(1));
    for scan in 0..9 {
        watchlist.record_scan(REGISTERED_AT + scan * MINUTE_NANOS, &account(1));
    }

    watchlist.record_scan(VALID_UNTIL - HOUR_NANOS, &account(1));
    assert_eq!(
        watchlist
            .get(VALID_UNTIL, &account(1))
            .unwrap()
            .next_scan_at,
        Some(VALID_UNTIL)
    );

    watchlist.record_scan(VALID_UNTIL - HOUR_NANOS + 1, &account(1));
    assert_eq!(
        watchlist
            .get(VALID_UNTIL, &account(1))
            .unwrap()
            .next_scan_at,
        None
    );
}

#[test]
fn should_ignore_a_scan_of_an_account_that_is_not_watched() {
    let mut watchlist = Watchlist::default();
    register(&mut watchlist, REGISTERED_AT, account(1));

    watchlist.record_scan(REGISTERED_AT, &account(2));
    watchlist.record_scan(VALID_UNTIL + 1, &account(1));

    assert_eq!(
        watchlist.iter().collect::<Vec<_>>(),
        vec![unscanned(account(1))]
    );
}

#[test]
fn should_remove_a_registration() {
    let mut watchlist = Watchlist::default();
    register(&mut watchlist, REGISTERED_AT, account(1));
    register(&mut watchlist, REGISTERED_AT, account(2));

    watchlist.remove(&account(1));
    watchlist.remove(&account(3));

    assert_eq!(watchlist.get(REGISTERED_AT, &account(1)), None);
    assert_eq!(watchlist.len(), 1);
    assert_eq!(watchlist.due(REGISTERED_AT), vec![unscanned(account(2))]);
}

#[test]
fn should_count_the_live_registrations_and_report_the_oldest() {
    let watchlist = watchlist_of_two_registrations_an_hour_apart();

    assert_eq!(watchlist.live_len(VALID_UNTIL), 2);
    assert_eq!(
        watchlist.oldest_registered_at(VALID_UNTIL),
        Some(REGISTERED_AT)
    );
    assert_eq!(watchlist.live_len(VALID_UNTIL + 1), 1);
    assert_eq!(
        watchlist.oldest_registered_at(VALID_UNTIL + 1),
        Some(REGISTERED_AT + HOUR_NANOS)
    );
    assert_eq!(watchlist.live_len(VALID_UNTIL + HOUR_NANOS + 1), 0);
    assert_eq!(
        watchlist.oldest_registered_at(VALID_UNTIL + HOUR_NANOS + 1),
        None
    );
    assert_eq!(watchlist.len(), 2);
}

#[test]
fn should_evict_the_registrations_whose_window_has_closed() {
    let mut watchlist = watchlist_of_two_registrations_an_hour_apart();

    assert_eq!(watchlist.evict_expired(VALID_UNTIL), 0);
    assert_eq!(watchlist.len(), 2);
    assert_eq!(watchlist.evict_expired(VALID_UNTIL + 1), 1);
    assert_eq!(watchlist.len(), 1);
    assert_eq!(watchlist.evict_expired(VALID_UNTIL + HOUR_NANOS + 1), 1);
    assert!(watchlist.is_empty());
}

#[test]
fn should_iterate_by_ascending_window_end() {
    let mut watchlist = Watchlist::default();
    register(&mut watchlist, REGISTERED_AT + HOUR_NANOS, account(1));
    register(&mut watchlist, REGISTERED_AT, account(3));
    register(&mut watchlist, REGISTERED_AT, account(2));

    let accounts: Vec<Account> = watchlist.iter().map(|watched| watched.account).collect();

    assert_eq!(accounts, vec![account(3), account(2), account(1)]);
}

fn full_watchlist() -> Watchlist {
    let mut watchlist = Watchlist::default();
    for index in 0..MAX_WATCHED_ADDRESSES.get() {
        register(&mut watchlist, REGISTERED_AT, account(index));
    }
    watchlist
}

fn watchlist_of_two_registrations_an_hour_apart() -> Watchlist {
    let mut watchlist = Watchlist::default();
    register(&mut watchlist, REGISTERED_AT, account(1));
    register(&mut watchlist, REGISTERED_AT + HOUR_NANOS, account(2));
    watchlist
}

fn unscanned(account: Account) -> WatchedAddress {
    WatchedAddress {
        account,
        address: deposit_address(account),
        valid_until: VALID_UNTIL,
        scan_count: 0,
        last_scanned_at: None,
        next_scan_at: Some(REGISTERED_AT),
    }
}
