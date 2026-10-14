use crate::{
    lifecycle::pre_upgrade,
    state::{
        event::{EventType, WatchlistSnapshot},
        mutate_state, read_state,
        watchlist::MAX_WATCHED_ADDRESSES,
    },
    test_fixtures::{
        EventsAssert, REGISTERED_AT, WATCH_WINDOW_NANOS, account, init_state,
        runtime::TestCanisterRuntime, watch,
    },
};

#[test]
fn should_record_an_empty_snapshot_on_every_upgrade() {
    init_state();

    pre_upgrade(TestCanisterRuntime::new().add_times([REGISTERED_AT, REGISTERED_AT]));

    EventsAssert::from_recorded()
        .expect_event_eq(EventType::WatchedDepositAddresses(WatchlistSnapshot {
            window_nanos: WATCH_WINDOW_NANOS,
            capacity: MAX_WATCHED_ADDRESSES.get() as u64,
            entries: vec![],
        }))
        .assert_no_more_events();
}

#[test]
fn should_record_the_watchlist_without_evicting_expired_registrations() {
    init_state();
    watch(REGISTERED_AT, account(1));
    mutate_state(|s| {
        s.watchlist_mut()
            .record_scan(REGISTERED_AT + 1, &account(1));
    });
    watch(REGISTERED_AT + 2, account(2));
    let watchlist_before = read_state(|s| s.watchlist().snapshot());
    let long_after_the_window = REGISTERED_AT + 10 * WATCH_WINDOW_NANOS;

    pre_upgrade(
        TestCanisterRuntime::new().add_times([long_after_the_window, long_after_the_window]),
    );

    assert_eq!(watchlist_before.entries.len(), 2);
    EventsAssert::from_recorded()
        .expect_event_eq(EventType::WatchedDepositAddresses(watchlist_before.clone()))
        .assert_no_more_events();
    assert_eq!(read_state(|s| s.watchlist().snapshot()), watchlist_before);
}
