use super::*;
use crate::{
    constants::MAX_CONCURRENT_RPC_CALLS,
    state::{event::EventType, read_state, reset_state},
    storage::{reset_automatic_deposit_cache, reset_events, with_automatic_deposit_cache},
    test_fixtures::{
        EventsAssert, account, events::start_monitoring_account, init_schnorr_master_key,
        init_state, runtime::TestCanisterRuntime,
    },
};
use sol_rpc_types::{ConfirmedTransactionStatusWithSignature, MultiRpcResult};

const ONE_MIN_NS: u64 = Duration::from_mins(1).as_nanos() as u64;

type SignaturesResult = MultiRpcResult<Vec<ConfirmedTransactionStatusWithSignature>>;

fn empty_signatures_response() -> SignaturesResult {
    MultiRpcResult::Consistent(Ok(vec![]))
}

fn monitored_accounts_count() -> usize {
    read_state(|s| s.monitored_accounts().len())
}

fn cache_entry(account: Account) -> Option<(Option<u64>, AutomaticDepositCacheEntry)> {
    with_automatic_deposit_cache(|cache| {
        cache.get_with_index(&account).map(|(t, entry)| {
            let next_poll_at = if t == u64::MAX { None } else { Some(t) };
            (next_poll_at, entry)
        })
    })
}

fn start_monitoring_max_number_of_accounts() {
    for i in 0..MAX_MONITORED_ACCOUNTS {
        start_monitoring_account(account(i));
    }
}

mod update_balance {
    use super::*;

    #[test]
    fn should_register_account() {
        init_state();
        let runtime = TestCanisterRuntime::new().with_increasing_time();

        let result = update_balance(&runtime, account(1));
        assert_eq!(result, Ok(()));

        assert!(read_state(|s| s.monitored_accounts().contains(&account(1))));
        assert_eq!(monitored_accounts_count(), 1);
        assert!(cache_entry(account(1)).is_some());

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
}

mod poll_monitored_addresses {
    use super::*;

    #[tokio::test]
    async fn should_poll_monitored_addresses_in_rounds() {
        setup();

        // Seed MAX_CONCURRENT_RPC_CALLS + 1 accounts, all due immediately.
        let num_accounts = MAX_CONCURRENT_RPC_CALLS + 1;
        for i in 0..num_accounts {
            start_monitoring_account(account(i));
            with_automatic_deposit_cache_mut(|cache| {
                cache.insert(account(i), 0, AutomaticDepositCacheEntry::default());
            });
        }
        assert_eq!(monitored_accounts_count(), num_accounts);

        // Round 1: processes MAX_CONCURRENT_RPC_CALLS accounts (rescheduled into the future),
        // detects 1 more remaining → sets timer.
        let mut runtime = TestCanisterRuntime::new().with_increasing_time();
        for _ in 0..MAX_CONCURRENT_RPC_CALLS {
            runtime = runtime.add_stub_response(empty_signatures_response());
        }
        poll_monitored_addresses(runtime.clone()).await;

        // Accounts are rescheduled (not removed), so count stays the same.
        assert_eq!(monitored_accounts_count(), num_accounts);
        assert_eq!(runtime.set_timer_call_count(), 1);

        // Round 2: only the 1 unprocessed account is due → processes it, no set_timer.
        let runtime = TestCanisterRuntime::new()
            .with_increasing_time()
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

        let mut next_poll_at = t0;

        for i in 0u8..MAX_GET_SIGNATURES_FOR_ADDRESS_CALLS {
            next_poll_at += ONE_MIN_NS << i;

            EventsAssert::from_recorded()
                .expect_event_eq(EventType::StartedMonitoringAccount { account })
                .assert_no_more_events();

            // Just before next scheduled time: no JSON-RPC calls
            let runtime = TestCanisterRuntime::new().add_times([next_poll_at - 1]);
            poll_monitored_addresses(runtime).await;

            // Next scheduled time: one `getSignaturesForAddress` call
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
