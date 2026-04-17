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

fn fill_monitored_accounts_to_capacity() {
    for i in 0..MAX_MONITORED_ACCOUNTS as usize {
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

    update_balance(&runtime, account(1)).unwrap();
    let result = update_balance(&runtime, account(1));

    assert_eq!(result, Ok(()));
    assert_eq!(monitored_accounts_count(), 1);

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

    fill_monitored_accounts_to_capacity();
    assert_eq!(monitored_accounts_count(), MAX_MONITORED_ACCOUNTS as usize);

    let runtime = TestCanisterRuntime::new().with_increasing_time();
    let result = update_balance(&runtime, account(MAX_MONITORED_ACCOUNTS as usize + 1));
    assert_eq!(result, Err(UpdateBalanceError::QueueFull));
    assert_eq!(monitored_accounts_count(), MAX_MONITORED_ACCOUNTS as usize);
}

#[test]
fn should_not_return_queue_full_if_account_already_monitored() {
    init_state();

    fill_monitored_accounts_to_capacity();
    assert_eq!(monitored_accounts_count(), MAX_MONITORED_ACCOUNTS as usize);

    // Re-registering an already-monitored account should still return Ok
    let runtime = TestCanisterRuntime::new().with_increasing_time();
    let result = update_balance(&runtime, account(0));
    assert_eq!(result, Ok(()));
}
