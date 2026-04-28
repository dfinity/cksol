use super::*;
use crate::{
    constants::MAX_CONCURRENT_RPC_CALLS,
    state::{
        event::{DepositId, DepositSource, EventType},
        read_state,
    },
    test_fixtures::{
        AUTOMATED_DEPOSIT_FEE, EventsAssert, account,
        deposit::{
            DEPOSIT_AMOUNT, DEPOSITOR_ACCOUNT, legacy_deposit_transaction,
            legacy_deposit_transaction_signature,
        },
        events::start_monitoring_account,
        init_schnorr_master_key, init_state,
        runtime::TestCanisterRuntime,
        signature,
    },
};
use sol_rpc_types::{ConfirmedTransactionStatusWithSignature, RpcError, TransactionError};

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

/// Pushes signatures into the pending queue for the given account.
fn queue_pending_signatures(account: Account, sigs: impl IntoIterator<Item = Signature>) {
    PENDING_SIGNATURES.with(|p| {
        p.borrow_mut().entry(account).or_default().extend(sigs);
    });
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
            runtime = runtime.add_get_signatures_for_address_response(vec![]);
        }
        poll_monitored_addresses(runtime.clone()).await;

        assert_eq!(monitored_accounts_count(), 1);
        assert_eq!(runtime.set_timer_call_count(), 1);

        // Round 2: polls the remaining 1 account → no reschedule, queue empty.
        let runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .add_get_signatures_for_address_response(vec![]);
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
            .add_get_signatures_for_address_response(vec![confirmed_tx(s1), confirmed_tx(s2)]);

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
            .add_get_signatures_for_address_response(vec![confirmed_tx(s_ok), failed_tx(s_fail)]);

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
            .add_get_signatures_for_address_error(RpcError::ValidationError(
                "RPC error".to_string(),
            ));

        poll_monitored_addresses(runtime).await;

        assert_eq!(pending_signatures_for(&acc), vec![]);
    }

    fn setup() {
        init_state();
        init_schnorr_master_key();
    }
}

mod process_pending_signatures_tests {
    use super::*;

    #[tokio::test]
    async fn should_accept_valid_deposit() {
        setup();
        reset_pending_signatures();

        let sig = legacy_deposit_transaction_signature();
        queue_pending_signatures(DEPOSITOR_ACCOUNT, [sig]);

        let runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .add_get_transaction_response(legacy_deposit_transaction());

        process_pending_signatures(runtime).await;

        assert!(
            pending_signatures_for(&DEPOSITOR_ACCOUNT).is_empty(),
            "signature should have been consumed"
        );

        let deposit_id = DepositId {
            account: DEPOSITOR_ACCOUNT,
            signature: sig,
        };
        EventsAssert::from_recorded()
            .expect_event_eq(EventType::AcceptedDeposit {
                deposit_id,
                deposit_amount: DEPOSIT_AMOUNT,
                amount_to_mint: DEPOSIT_AMOUNT - AUTOMATED_DEPOSIT_FEE,
                source: DepositSource::Automatic,
            })
            .assert_no_more_events();
    }

    #[tokio::test]
    async fn should_discard_invalid_deposit() {
        setup();
        reset_pending_signatures();

        let sig = legacy_deposit_transaction_signature();
        queue_pending_signatures(DEPOSITOR_ACCOUNT, [sig]);

        // getTransaction returns None (tx not found)
        let runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .add_get_transaction_not_found();

        process_pending_signatures(runtime).await;

        assert!(pending_signatures_for(&DEPOSITOR_ACCOUNT).is_empty());
        EventsAssert::assert_no_events_recorded();
    }

    #[tokio::test]
    async fn should_skip_already_processed_deposit() {
        setup();
        reset_pending_signatures();

        let sig = legacy_deposit_transaction_signature();

        // Pre-populate state as if this deposit was already accepted manually.
        crate::test_fixtures::events::accept_deposit(
            DepositId {
                account: DEPOSITOR_ACCOUNT,
                signature: sig,
            },
            DEPOSIT_AMOUNT,
        );

        queue_pending_signatures(DEPOSITOR_ACCOUNT, [sig]);

        // No getTransaction stub — if called it would panic.
        let runtime = TestCanisterRuntime::new().with_increasing_time();

        process_pending_signatures(runtime).await;

        use crate::test_fixtures::MANUAL_DEPOSIT_FEE;
        // No new AcceptedDeposit (Automatic) event should have been emitted.
        EventsAssert::from_recorded()
            .expect_event_eq(EventType::AcceptedDeposit {
                deposit_id: DepositId {
                    account: DEPOSITOR_ACCOUNT,
                    signature: sig,
                },
                deposit_amount: DEPOSIT_AMOUNT,
                amount_to_mint: DEPOSIT_AMOUNT - MANUAL_DEPOSIT_FEE,
                source: DepositSource::Manual,
            })
            .assert_no_more_events();
    }

    #[tokio::test]
    async fn should_process_multiple_signatures_per_account_when_capacity_allows() {
        setup();
        reset_pending_signatures();

        // Two accounts with two signatures each — all four fit within MAX_CONCURRENT_RPC_CALLS.
        let acc1 = DEPOSITOR_ACCOUNT;
        let acc2 = account(2);
        let sigs: Vec<_> = (1..=4).map(signature).collect();

        queue_pending_signatures(acc1, [sigs[0], sigs[1]]);
        queue_pending_signatures(acc2, [sigs[2], sigs[3]]);

        // All four getTransaction calls return None (invalid deposit — just discard).
        let runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .add_n_get_transaction_not_found(4);

        process_pending_signatures(runtime.clone()).await;

        // All consumed in one call thanks to multi-pass round-robin; no reschedule.
        assert!(pending_signatures_for(&acc1).is_empty());
        assert!(pending_signatures_for(&acc2).is_empty());
        assert_eq!(runtime.set_timer_call_count(), 0);
        EventsAssert::assert_no_events_recorded();
    }

    #[tokio::test]
    async fn should_reschedule_when_capacity_exhausted() {
        setup();
        reset_pending_signatures();

        // MAX_CONCURRENT_RPC_CALLS + 1 signatures → only MAX fit in one round.
        let sigs: Vec<_> = (0..=MAX_CONCURRENT_RPC_CALLS).map(signature).collect();
        queue_pending_signatures(DEPOSITOR_ACCOUNT, sigs.iter().copied());

        let runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .add_n_get_transaction_not_found(MAX_CONCURRENT_RPC_CALLS);

        process_pending_signatures(runtime.clone()).await;

        // Last signature still pending → reschedule.
        assert_eq!(
            pending_signatures_for(&DEPOSITOR_ACCOUNT),
            vec![sigs[MAX_CONCURRENT_RPC_CALLS]]
        );
        assert_eq!(runtime.set_timer_call_count(), 1);
    }

    fn setup() {
        init_state();
        init_schnorr_master_key();
    }
}
