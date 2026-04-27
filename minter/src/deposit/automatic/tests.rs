use super::*;
use crate::{
    constants::MAX_CONCURRENT_RPC_CALLS,
    state::{event::EventType, read_state},
    test_fixtures::{
        EventsAssert, account, events::start_monitoring_account, init_schnorr_master_key,
        init_state, runtime::TestCanisterRuntime, signature,
    },
};
use sol_rpc_types::{ConfirmedTransactionStatusWithSignature, MultiRpcResult, TransactionError};

type SignaturesResult = MultiRpcResult<Vec<ConfirmedTransactionStatusWithSignature>>;

fn confirmed_tx(signature: Signature) -> ConfirmedTransactionStatusWithSignature {
    ConfirmedTransactionStatusWithSignature {
        signature: signature.into(),
        slot: 12345,
        err: None,
        memo: None,
        block_time: None,
        confirmation_status: None,
    }
}

fn failed_tx(signature: Signature) -> ConfirmedTransactionStatusWithSignature {
    ConfirmedTransactionStatusWithSignature {
        signature: signature.into(),
        slot: 12345,
        err: Some(TransactionError::AccountNotFound),
        memo: None,
        block_time: None,
        confirmation_status: None,
    }
}

fn monitored_accounts_count() -> usize {
    read_state(|s| s.monitored_accounts().len())
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

        // Add MAX_CONCURRENT_RPC_CALLS + 1 accounts to monitor so that 2 rounds are needed.
        let num_accounts = MAX_CONCURRENT_RPC_CALLS + 1;
        for i in 0..num_accounts {
            start_monitoring_account(account(i));
        }
        assert_eq!(monitored_accounts_count(), num_accounts);

        // Round 1: polls MAX_CONCURRENT_RPC_CALLS accounts, 1 remains → reschedule.
        let mut runtime = TestCanisterRuntime::new().with_increasing_time();
        for _ in 0..MAX_CONCURRENT_RPC_CALLS {
            runtime = runtime.add_stub_response(SignaturesResult::Consistent(Ok(vec![])));
        }
        poll_monitored_addresses(runtime.clone()).await;

        assert_eq!(monitored_accounts_count(), 1);
        assert_eq!(runtime.set_timer_call_count(), 1);

        // Round 2: polls the remaining 1 account → no reschedule, queue empty.
        let runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .add_stub_response(SignaturesResult::Consistent(Ok(vec![])));
        poll_monitored_addresses(runtime.clone()).await;

        assert_eq!(monitored_accounts_count(), 0);
        assert_eq!(runtime.set_timer_call_count(), 0);

        // Verify StoppedMonitoringAccount was emitted for each account.
        let mut events_assert = EventsAssert::from_recorded();
        for i in 0..num_accounts {
            events_assert =
                events_assert.expect_contains_event_eq(EventType::StoppedMonitoringAccount {
                    account: account(i),
                });
        }
    }

    #[tokio::test]
    async fn should_queue_discovered_signatures() {
        setup();
        reset_pending_signatures();

        let acc = account(1);
        start_monitoring_account(acc);

        let s1 = signature(1);
        let s2 = signature(2);
        let runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .add_stub_response(SignaturesResult::Consistent(Ok(vec![
                confirmed_tx(s1),
                confirmed_tx(s2),
            ])));

        poll_monitored_addresses(runtime).await;

        assert_eq!(pending_signatures_for(&acc), vec![s1, s2]);
    }

    #[tokio::test]
    async fn should_not_queue_failed_transactions() {
        setup();
        reset_pending_signatures();

        let acc = account(1);
        start_monitoring_account(acc);

        let s_ok = signature(1);
        let s_fail = signature(2);
        let runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .add_stub_response(SignaturesResult::Consistent(Ok(vec![
                confirmed_tx(s_ok),
                failed_tx(s_fail),
            ])));

        poll_monitored_addresses(runtime).await;

        assert_eq!(pending_signatures_for(&acc), vec![s_ok]);
    }

    #[tokio::test]
    async fn should_not_queue_signatures_if_rpc_call_fails() {
        setup();
        reset_pending_signatures();

        let acc = account(1);
        start_monitoring_account(acc);

        let runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .add_stub_response(SignaturesResult::Consistent(Err(
                sol_rpc_types::RpcError::ValidationError("RPC error".to_string()),
            )));

        poll_monitored_addresses(runtime).await;

        assert_eq!(pending_signatures_for(&acc), vec![]);
    }

    fn setup() {
        init_state();
        init_schnorr_master_key();
    }
}
