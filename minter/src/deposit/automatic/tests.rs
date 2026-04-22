use super::*;
use crate::{
    constants::MAX_CONCURRENT_RPC_CALLS,
    state::{event::EventType, read_state, reset_state},
    storage::{reset_automatic_deposit_cache, reset_events, with_automatic_deposit_cache},
    test_fixtures::{
        EventsAssert, account, init_schnorr_master_key, init_state, runtime::TestCanisterRuntime,
    },
};
use cache::{INITIAL_BACKOFF_DELAY_MINS, INITIAL_RPC_QUOTA};
use candid::Principal;
use sol_rpc_types::{ConfirmedTransactionStatusWithSignature, MultiRpcResult};
use std::iter;

type SignaturesResult = MultiRpcResult<Vec<ConfirmedTransactionStatusWithSignature>>;

mod update_balance_tests {
    use super::*;

    const ACCOUNT: Account = Account {
        owner: Principal::from_slice(&[1; 29]),
        subaccount: None,
    };

    #[test]
    fn should_start_monitoring_unknown_account() {
        init_state();
        let runtime = TestCanisterRuntime::new().add_times(iter::repeat(0));

        let result = update_balance(&runtime, ACCOUNT);
        assert_eq!(result, Ok(()));

        // Start monitoring the account and add a cache entry with a fresh quota and backoff delay
        CacheAssert::for_account(ACCOUNT)
            .next_poll_at_mins(INITIAL_BACKOFF_DELAY_MINS)
            .quota(INITIAL_RPC_QUOTA)
            .backoff_mins(INITIAL_BACKOFF_DELAY_MINS);
        EventsAssert::from_recorded()
            .expect_event_eq(EventType::StartedMonitoringAccount { account: ACCOUNT })
            .assert_no_more_events();
    }

    #[test]
    fn should_not_modify_active_account() {
        init_state();
        let runtime = TestCanisterRuntime::new().add_times(iter::repeat(0));

        let result1 = update_balance(&runtime, ACCOUNT);
        assert_eq!(result1, Ok(()));

        set_cache_entry(ACCOUNT, 1, 7, 4);
        let cache_before = CacheAssert::for_account(ACCOUNT);
        let events_before = EventsAssert::from_recorded();

        let result2 = update_balance(&runtime, ACCOUNT);
        assert_eq!(result2, Ok(()));

        // Does not modify the cache or events
        assert_eq!(cache_before, CacheAssert::for_account(ACCOUNT));
        assert_eq!(events_before, EventsAssert::from_recorded());
    }

    #[test]
    fn should_reschedule_stopped_account() {
        init_state();
        let runtime = TestCanisterRuntime::new().add_times(iter::repeat(0));

        set_cache_entry(ACCOUNT, u64::MAX, 5, 8);

        let result = update_balance(&runtime, ACCOUNT);
        assert_eq!(result, Ok(()));

        // Does not modify the quota but resets the backoff delay and starts monitoring
        CacheAssert::for_account(ACCOUNT)
            .next_poll_at_mins(INITIAL_BACKOFF_DELAY_MINS)
            .quota(5)
            .backoff_mins(INITIAL_BACKOFF_DELAY_MINS);
        EventsAssert::from_recorded()
            .expect_event_eq(EventType::StartedMonitoringAccount { account: ACCOUNT })
            .assert_no_more_events();
    }

    #[test]
    fn should_return_error_for_exhausted_account() {
        init_state();
        let runtime = TestCanisterRuntime::new().add_times(iter::repeat(0));

        set_cache_entry(ACCOUNT, u64::MAX, 0, INITIAL_BACKOFF_DELAY_MINS);
        let cache_before = CacheAssert::for_account(ACCOUNT);
        let events_before = EventsAssert::from_recorded();

        let result = update_balance(&runtime, ACCOUNT);
        assert_eq!(result, Err(UpdateBalanceError::MonitoringQuotaExhausted));

        // Does not modify the cache or events
        assert_eq!(cache_before, CacheAssert::for_account(ACCOUNT));
        assert_eq!(events_before, EventsAssert::from_recorded());
    }

    #[test]
    fn should_return_error_when_at_capacity() {
        init_state();
        start_monitoring_max_number_of_accounts();
        let runtime = TestCanisterRuntime::new().add_times(iter::repeat(0));

        // Account is not being monitored, return error
        let result1 = update_balance(&runtime, account(MAX_MONITORED_ACCOUNTS + 1));
        assert_eq!(result1, Err(UpdateBalanceError::QueueFull));

        // Account is already being monitored, return ok
        let result2 = update_balance(&runtime, account(0));
        assert_eq!(result2, Ok(()));
    }
}

mod poll_monitored_addresses_tests {
    use super::*;

    #[tokio::test]
    async fn should_poll_monitored_addresses_in_rounds() {
        setup();

        // Seed MAX_CONCURRENT_RPC_CALLS + 1 accounts, all due immediately.
        let num_accounts = MAX_CONCURRENT_RPC_CALLS + 1;
        let runtime = TestCanisterRuntime::new().add_times(0..);
        for i in 0..num_accounts {
            update_balance(&runtime, account(i)).unwrap();
            set_cache_entry(account(i), 0, INITIAL_RPC_QUOTA, INITIAL_BACKOFF_DELAY_MINS);
        }

        // Round 1: processes MAX_CONCURRENT_RPC_CALLS accounts (rescheduled into the future),
        // detects 1 more remaining → sets timer.
        let mut runtime = TestCanisterRuntime::new().add_times(0..);
        for _ in 0..MAX_CONCURRENT_RPC_CALLS {
            runtime = runtime.add_stub_response(empty_signatures_response());
        }
        poll_monitored_addresses(runtime.clone()).await;

        assert_eq!(monitored_accounts_count(), num_accounts);
        assert_eq!(runtime.set_timer_call_count(), 1);

        // Round 2: only the 1 unprocessed account is due → processes it, no set_timer.
        let runtime = TestCanisterRuntime::new()
            .add_times(0..)
            .add_stub_response(empty_signatures_response());
        poll_monitored_addresses(runtime.clone()).await;

        assert_eq!(monitored_accounts_count(), num_accounts);
        assert_eq!(runtime.set_timer_call_count(), 0);
    }

    #[tokio::test]
    async fn should_poll_signatures_for_address_with_exponential_backoff() {
        setup();
        let t0 = 0u64;

        let account = account(1);
        let runtime = TestCanisterRuntime::new().add_times([t0, t0, t0]);
        update_balance(&runtime, account).unwrap();

        for _ in 0..INITIAL_RPC_QUOTA {
            let next_poll_at = CacheAssert::for_account(account).scheduled_at();

            EventsAssert::from_recorded()
                .expect_event_eq(EventType::StartedMonitoringAccount { account })
                .assert_no_more_events();

            // Just before next scheduled time: no JSON-RPC calls.
            let runtime = TestCanisterRuntime::new().add_times([next_poll_at - 1]);
            poll_monitored_addresses(runtime).await;

            // At the scheduled time: one `getSignaturesForAddress` call.
            let runtime = TestCanisterRuntime::new()
                .add_times([next_poll_at; 3])
                .add_stub_response(empty_signatures_response());
            poll_monitored_addresses(runtime).await;
        }

        EventsAssert::from_recorded()
            .expect_event_eq(EventType::StartedMonitoringAccount { account })
            .expect_event_eq(EventType::StoppedMonitoringAccount { account })
            .assert_no_more_events();
    }

    fn setup() {
        reset_state();
        reset_events();
        reset_automatic_deposit_cache();
        init_state();
        init_schnorr_master_key();
    }
}

fn empty_signatures_response() -> SignaturesResult {
    MultiRpcResult::Consistent(Ok(vec![]))
}

fn monitored_accounts_count() -> usize {
    read_state(|s| s.monitored_accounts().len())
}

fn start_monitoring_max_number_of_accounts() {
    let runtime = TestCanisterRuntime::new().add_times(0..);
    for i in 0..MAX_MONITORED_ACCOUNTS {
        update_balance(&runtime, account(i)).unwrap();
    }
}

fn set_cache_entry(account: Account, next_poll_at: u64, quota: u64, backoff_mins: u64) {
    with_automatic_deposit_cache_mut(|cache| {
        cache.insert(
            account,
            next_poll_at,
            AutomaticDepositCacheEntry {
                rpc_quota_left: quota,
                next_backoff_delay_mins: backoff_mins,
            },
        );
    });
}

#[derive(Debug, PartialEq)]
struct CacheAssert {
    account: Account,
    next_poll_at: Option<u64>,
    entry: AutomaticDepositCacheEntry,
}

impl CacheAssert {
    fn for_account(account: Account) -> Self {
        let (next_poll_at, entry) = with_automatic_deposit_cache(|cache| {
            cache.get_with_index(&account).map(|(t, entry)| {
                let next_poll_at = if t == u64::MAX { None } else { Some(t) };
                (next_poll_at, entry)
            })
        })
        .unwrap_or_else(|| panic!("No cache entry for account {account:?}"));
        Self {
            account,
            next_poll_at,
            entry,
        }
    }

    /// Returns the scheduled poll time, panicking if the account is stopped.
    fn scheduled_at(self) -> u64 {
        self.next_poll_at.unwrap_or_else(|| {
            panic!(
                "Account {:?} is stopped (no scheduled poll time)",
                self.account
            )
        })
    }

    /// Asserts that the account is scheduled exactly `delay_mins` minutes from time 0.
    fn next_poll_at_mins(self, delay_mins: u64) -> Self {
        let expected = Duration::from_mins(delay_mins).as_nanos() as u64;
        assert_eq!(
            self.next_poll_at,
            Some(expected),
            "next_poll_at mismatch for account {:?}",
            self.account
        );
        self
    }

    fn quota(self, expected: u64) -> Self {
        assert_eq!(
            self.entry.rpc_quota_left, expected,
            "rpc_quota_left mismatch for account {:?}",
            self.account
        );
        self
    }

    fn backoff_mins(self, expected: u64) -> Self {
        assert_eq!(
            self.entry.next_backoff_delay_mins, expected,
            "next_backoff_delay_mins mismatch for account {:?}",
            self.account
        );
        self
    }
}
