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
use sol_rpc_types::{
    ConfirmedTransactionStatusWithSignature, EncodedConfirmedTransactionWithStatusMeta,
    MultiRpcResult, TransactionError,
};

type GetTransactionResult = MultiRpcResult<Option<EncodedConfirmedTransactionWithStatusMeta>>;

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

mod process_pending_signatures_tests {
    use super::*;

    #[tokio::test]
    async fn should_accept_valid_deposit() {
        setup();
        reset_pending_signatures();

        let signature = legacy_deposit_transaction_signature();
        PENDING_SIGNATURES.with(|p| {
            p.borrow_mut()
                .entry(DEPOSITOR_ACCOUNT)
                .or_default()
                .push_back(signature);
        });

        let get_tx_response = GetTransactionResult::Consistent(Ok(Some(
            legacy_deposit_transaction().try_into().unwrap(),
        )));
        let runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .add_stub_response(get_tx_response);

        process_pending_signatures(runtime).await;

        assert!(
            pending_signatures_for(&DEPOSITOR_ACCOUNT).is_empty(),
            "signature should have been consumed"
        );

        let deposit_id = DepositId {
            account: DEPOSITOR_ACCOUNT,
            signature,
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

        let signature = legacy_deposit_transaction_signature();
        PENDING_SIGNATURES.with(|p| {
            p.borrow_mut()
                .entry(DEPOSITOR_ACCOUNT)
                .or_default()
                .push_back(signature);
        });

        // getTransaction returns None (tx not found)
        let runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .add_stub_response(GetTransactionResult::Consistent(Ok(None)));

        process_pending_signatures(runtime).await;

        assert!(pending_signatures_for(&DEPOSITOR_ACCOUNT).is_empty());
        EventsAssert::assert_no_events_recorded();
    }

    #[tokio::test]
    async fn should_skip_already_processed_deposit() {
        setup();
        reset_pending_signatures();

        let signature = legacy_deposit_transaction_signature();

        // Pre-populate state as if this deposit was already accepted manually.
        crate::test_fixtures::events::accept_deposit(
            DepositId {
                account: DEPOSITOR_ACCOUNT,
                signature,
            },
            DEPOSIT_AMOUNT,
        );

        PENDING_SIGNATURES.with(|p| {
            p.borrow_mut()
                .entry(DEPOSITOR_ACCOUNT)
                .or_default()
                .push_back(signature);
        });

        // No getTransaction stub — if called it would panic.
        let runtime = TestCanisterRuntime::new().with_increasing_time();

        process_pending_signatures(runtime).await;

        use crate::test_fixtures::MANUAL_DEPOSIT_FEE;
        // No new AcceptedDeposit (Automatic) event should have been emitted.
        EventsAssert::from_recorded()
            .expect_event_eq(EventType::AcceptedDeposit {
                deposit_id: DepositId {
                    account: DEPOSITOR_ACCOUNT,
                    signature,
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

        PENDING_SIGNATURES.with(|p| {
            let mut p = p.borrow_mut();
            p.entry(acc1).or_default().extend([sigs[0], sigs[1]]);
            p.entry(acc2).or_default().extend([sigs[2], sigs[3]]);
        });

        // All four getTransaction calls return None (invalid deposit — just discard).
        let mut runtime = TestCanisterRuntime::new().with_increasing_time();
        for _ in 0..4 {
            runtime = runtime.add_stub_response(GetTransactionResult::Consistent(Ok(None)));
        }

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
        PENDING_SIGNATURES.with(|p| {
            p.borrow_mut()
                .entry(DEPOSITOR_ACCOUNT)
                .or_default()
                .extend(sigs.iter().copied());
        });

        let mut runtime = TestCanisterRuntime::new().with_increasing_time();
        for _ in 0..MAX_CONCURRENT_RPC_CALLS {
            runtime = runtime.add_stub_response(GetTransactionResult::Consistent(Ok(None)));
        }

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

mod mint_automatic_deposits_tests {
    use super::*;
    use crate::{
        storage::reset_events,
        test_fixtures::{BLOCK_INDEX, events::accept_automatic_deposit},
    };
    use icrc_ledger_types::icrc1::transfer::{BlockIndex, TransferError};

    #[tokio::test]
    async fn should_mint_accepted_deposit() {
        setup();

        let deposit_id = DepositId {
            account: DEPOSITOR_ACCOUNT,
            signature: legacy_deposit_transaction_signature(),
        };
        let amount_to_mint = DEPOSIT_AMOUNT - AUTOMATED_DEPOSIT_FEE;
        accept_automatic_deposit(deposit_id, DEPOSIT_AMOUNT, amount_to_mint);
        reset_events();

        let runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .add_stub_response(Ok::<BlockIndex, TransferError>(BLOCK_INDEX.into()));

        mint_automatic_deposits(runtime).await;

        assert!(
            read_state(|s| s.accepted_deposits().is_empty()),
            "deposit should have been minted"
        );
        EventsAssert::from_recorded()
            .expect_event_eq(EventType::Minted {
                deposit_id,
                mint_block_index: BLOCK_INDEX.into(),
            })
            .assert_no_more_events();
    }

    #[tokio::test]
    async fn should_keep_deposit_on_mint_failure() {
        setup();

        let deposit_id = DepositId {
            account: DEPOSITOR_ACCOUNT,
            signature: legacy_deposit_transaction_signature(),
        };
        let amount_to_mint = DEPOSIT_AMOUNT - AUTOMATED_DEPOSIT_FEE;
        accept_automatic_deposit(deposit_id, DEPOSIT_AMOUNT, amount_to_mint);

        let runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .add_stub_response(Err::<BlockIndex, TransferError>(
                TransferError::TemporarilyUnavailable,
            ));

        mint_automatic_deposits(runtime).await;

        // Deposit should still be in accepted_deposits for retry.
        assert_eq!(read_state(|s| s.accepted_deposits().len()), 1);
    }

    #[tokio::test]
    async fn should_do_nothing_when_no_accepted_deposits() {
        setup();

        let runtime = TestCanisterRuntime::new().with_increasing_time();
        mint_automatic_deposits(runtime).await;

        EventsAssert::assert_no_events_recorded();
    }

    #[tokio::test]
    async fn should_reschedule_when_more_deposits_than_capacity() {
        setup();

        // Accept MAX_CONCURRENT_RPC_CALLS + 1 automatic deposits.
        let amount_to_mint = DEPOSIT_AMOUNT - AUTOMATED_DEPOSIT_FEE;
        for i in 0..=MAX_CONCURRENT_RPC_CALLS {
            let deposit_id = DepositId {
                account: DEPOSITOR_ACCOUNT,
                signature: signature(i),
            };
            accept_automatic_deposit(deposit_id, DEPOSIT_AMOUNT, amount_to_mint);
        }
        reset_events();

        // Provide exactly MAX_CONCURRENT_RPC_CALLS mint stubs.
        let mut runtime = TestCanisterRuntime::new().with_increasing_time();
        for i in 0..MAX_CONCURRENT_RPC_CALLS {
            runtime = runtime.add_stub_response(Ok::<BlockIndex, TransferError>(
                (BLOCK_INDEX + i as u64).into(),
            ));
        }

        mint_automatic_deposits(runtime.clone()).await;

        // One deposit remains → reschedule.
        assert_eq!(read_state(|s| s.accepted_deposits().len()), 1);
        assert_eq!(runtime.set_timer_call_count(), 1);
    }

    #[tokio::test]
    async fn should_skip_manual_deposits() {
        use crate::test_fixtures::events::accept_deposit;

        setup();

        let deposit_id = DepositId {
            account: DEPOSITOR_ACCOUNT,
            signature: legacy_deposit_transaction_signature(),
        };
        accept_deposit(deposit_id, DEPOSIT_AMOUNT);

        let runtime = TestCanisterRuntime::new().with_increasing_time();
        mint_automatic_deposits(runtime).await;

        // Manual deposit should not have been minted by this timer.
        assert_eq!(
            read_state(|s| s.accepted_deposits().len()),
            1,
            "manual deposit should remain in accepted_deposits"
        );
    }

    fn setup() {
        init_state();
        init_schnorr_master_key();
    }
}
