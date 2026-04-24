use super::*;
use crate::{
    constants::MAX_CONCURRENT_RPC_CALLS,
    state::{event::EventType, read_state},
    storage::{reset_discovered_signatures, with_discovered_signatures},
    test_fixtures::{
        EventsAssert, account, events::start_monitoring_account, init_schnorr_master_key,
        init_state, runtime::TestCanisterRuntime,
    },
};
use sol_rpc_types::{ConfirmedTransactionStatusWithSignature, MultiRpcResult};

type SignaturesResult = MultiRpcResult<Vec<ConfirmedTransactionStatusWithSignature>>;

fn monitored_accounts_count() -> usize {
    read_state(|s| s.monitored_accounts().len())
}

fn start_monitoring_max_number_of_accounts() {
    for i in 0..MAX_MONITORED_ACCOUNTS {
        start_monitoring_account(account(i));
    }
}

fn confirmed_tx_status(signature_bytes: [u8; 64]) -> ConfirmedTransactionStatusWithSignature {
    ConfirmedTransactionStatusWithSignature {
        signature: solana_signature::Signature::from(signature_bytes).into(),
        slot: 12345,
        err: None,
        memo: None,
        block_time: None,
        confirmation_status: None,
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
    async fn should_enqueue_discovered_signatures() {
        setup();

        let account = account(0);
        start_monitoring_account(account);

        let sig1: [u8; 64] = [1u8; 64];
        let sig2: [u8; 64] = [2u8; 64];
        let runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .add_stub_response(SignaturesResult::Consistent(Ok(vec![
                confirmed_tx_status(sig1),
                confirmed_tx_status(sig2),
            ])));

        poll_monitored_addresses(runtime).await;

        with_discovered_signatures(|queue| {
            assert_eq!(queue.get(&sig1), Some(account));
            assert_eq!(queue.get(&sig2), Some(account));
            assert_eq!(queue.len(), 2);
        });
    }

    #[tokio::test]
    async fn should_not_enqueue_signatures_on_rpc_failure() {
        setup();

        let account = account(0);
        start_monitoring_account(account);

        let runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .add_stub_response(SignaturesResult::Consistent(Err(
                sol_rpc_types::RpcError::ProviderError(
                    sol_rpc_types::ProviderError::InvalidRpcConfig("test".to_string()),
                ),
            )));

        poll_monitored_addresses(runtime).await;

        with_discovered_signatures(|queue| {
            assert_eq!(queue.len(), 0);
        });
    }

    fn setup() {
        init_state();
        init_schnorr_master_key();
        reset_discovered_signatures();
    }
}
