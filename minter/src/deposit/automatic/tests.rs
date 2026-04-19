use super::*;
use crate::{
    state::{event::EventType, read_state},
    test_fixtures::{
        EventsAssert, account, events::start_monitoring_account, init_state,
        runtime::TestCanisterRuntime,
    },
};

fn monitored_accounts_count() -> usize {
    read_state(|s| s.monitored_accounts().len())
}

fn start_monitoring_max_number_of_accounts() {
    for i in 0..MAX_MONITORED_ACCOUNTS {
        start_monitoring_account(account(i));
    }
}

#[test]
fn should_register_account() {
    init_state();
    let runtime = TestCanisterRuntime::new().with_increasing_time();

    let result = update_balance(&runtime, account(1));
    assert_eq!(result, Ok(()));

    assert!(read_state(|s| s.monitored_accounts().contains(&account(1))));
    assert_eq!(monitored_accounts_count(), 1);

    EventsAssert::from_recorded()
        .expect_event_eq(EventType::StartedMonitoringAccount {
            account: account(1),
        })
        .assert_no_more_events();
}

#[test]
fn should_be_idempotent_for_already_monitored_account() {
    init_state();
    let runtime = TestCanisterRuntime::new().with_increasing_time();

    for _ in 0..2 {
        let result = update_balance(&runtime, account(1));
        assert_eq!(result, Ok(()));
        assert_eq!(monitored_accounts_count(), 1);
    }

    // Only one event should have been emitted
    EventsAssert::from_recorded()
        .expect_event_eq(EventType::StartedMonitoringAccount {
            account: account(1),
        })
        .assert_no_more_events();
}

#[test]
fn should_return_queue_full_when_at_capacity() {
    init_state();

    start_monitoring_max_number_of_accounts();
    assert_eq!(monitored_accounts_count(), MAX_MONITORED_ACCOUNTS);

    let runtime = TestCanisterRuntime::new().with_increasing_time();
    let result = update_balance(&runtime, account(MAX_MONITORED_ACCOUNTS + 1));
    assert_eq!(result, Err(UpdateBalanceError::QueueFull));
    assert_eq!(monitored_accounts_count(), MAX_MONITORED_ACCOUNTS);
}

#[test]
fn should_not_return_queue_full_if_account_already_monitored() {
    init_state();

    start_monitoring_max_number_of_accounts();
    assert_eq!(monitored_accounts_count(), MAX_MONITORED_ACCOUNTS);

    // Re-registering an already-monitored account should still return Ok
    let runtime = TestCanisterRuntime::new().with_increasing_time();
    let result = update_balance(&runtime, account(0));
    assert_eq!(result, Ok(()));
}
