use super::{
    FINALIZE_TRANSACTIONS_RETRY_DELAY, MAX_BLOCKHASH_AGE_IN_BLOCKS,
    MAX_SIGNATURES_PER_STATUS_CHECK, MIN_REBROADCAST_AGE, finalize_transactions,
};
use crate::{
    constants::MAX_CONCURRENT_RPC_CALLS,
    rpc::BlockHeight,
    state::{
        MinterTransaction, TaskType,
        event::{EventType, VersionedMessage},
        mutate_state, read_state, reset_state,
    },
    storage::{reset_events, with_unstable_metrics},
    test_fixtures::{
        EventsAssert, GetTransactionResult, MINIMUM_WITHDRAWAL_AMOUNT, MINTER_ADDRESS,
        NONCE_ACCOUNT, account, confirmed_block_at_height, durable_nonce, events,
        failed_withdrawal_response, finalized_status, init_balance, init_schnorr_master_key,
        init_state, nonce_account_info, runtime::TestCanisterRuntime, signature,
        succeeded_withdrawal_response,
    },
};
use sol_rpc_types::{
    ConfirmedBlock, MultiRpcResult, RpcError, SendTransactionParams, Slot,
    TransactionConfirmationStatus, TransactionError, TransactionStatus,
};
use solana_transaction::Transaction;

type SlotResult = MultiRpcResult<Slot>;
type BlockResult = MultiRpcResult<ConfirmedBlock>;
type SignatureStatusesResult = MultiRpcResult<Vec<Option<TransactionStatus>>>;
type SendTransactionResult = MultiRpcResult<sol_rpc_types::Signature>;
type GetAccountInfoResult = MultiRpcResult<Option<sol_rpc_types::AccountInfo>>;

const CURRENT_SLOT: Slot = 408_807_102;
const SUBMISSION_SLOT: Slot = CURRENT_SLOT - 10;
const CURRENT_BLOCK_HEIGHT: BlockHeight = BlockHeight::new(CURRENT_SLOT - 1_000);
const OLDEST_VALID_BLOCK_HEIGHT: BlockHeight =
    BlockHeight::new(CURRENT_BLOCK_HEIGHT.get() - MAX_BLOCKHASH_AGE_IN_BLOCKS.get());
const EXPIRED_BLOCK_HEIGHT: BlockHeight = BlockHeight::new(OLDEST_VALID_BLOCK_HEIGHT.get() - 1);

mod finalization {
    use super::*;

    #[tokio::test]
    async fn should_return_early_if_no_submitted_transactions() {
        setup();
        let events_before = EventsAssert::from_recorded();

        finalize_transactions(TestCanisterRuntime::new().with_increasing_time()).await;

        assert_eq!(EventsAssert::from_recorded(), events_before);
    }

    #[tokio::test]
    async fn should_return_early_if_task_already_active() {
        setup();
        submit_sweep_transaction(CURRENT_BLOCK_HEIGHT);

        mutate_state(|s| {
            s.active_tasks_mut().insert(TaskType::FinalizeTransactions);
        });

        let events_before = EventsAssert::from_recorded();

        finalize_transactions(TestCanisterRuntime::new()).await;

        let events_after = EventsAssert::from_recorded();
        assert_eq!(events_before, events_after);
    }

    #[tokio::test]
    async fn should_finalize_but_not_expire_transactions_if_fetching_current_block_fails() {
        setup();
        let finalized = submit_sweep_transaction_with_signature(1, EXPIRED_BLOCK_HEIGHT);
        let not_found = submit_sweep_transaction_with_signature(2, EXPIRED_BLOCK_HEIGHT);

        let runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .add_recent_block(Err(RpcError::ValidationError("Error".to_string())))
            .add_stub_response(SignatureStatusesResult::Consistent(Ok(vec![
                Some(finalized_status()),
                None,
            ])))
            .add_stub_response(GetTransactionResult::Consistent(Ok(None)));

        finalize_transactions(runtime).await;

        let events = EventsAssert::from_recorded().expect_contains_event_eq(
            EventType::SucceededTransaction {
                signature: finalized,
            },
        );
        assert!(!events.contains_event(&EventType::ExpiredTransaction {
            signature: not_found
        }));
        read_state(|s| assert!(s.submitted_transactions().contains_key(&not_found)));
    }

    #[tokio::test]
    async fn should_reschedule_while_more_sweeps_are_in_flight_than_a_round_checks() {
        setup();

        let num = MAX_CONCURRENT_RPC_CALLS * MAX_SIGNATURES_PER_STATUS_CHECK + 1;
        for i in 0..num {
            submit_sweep_transaction_with_signature(i, CURRENT_BLOCK_HEIGHT);
        }

        let mut runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .add_stub_response(SlotResult::Consistent(Ok(CURRENT_SLOT)))
            .add_stub_response(BlockResult::Consistent(Ok(current_block())));
        for _ in 0..MAX_CONCURRENT_RPC_CALLS {
            runtime = runtime.add_stub_response(SignatureStatusesResult::Consistent(Ok(vec![
                    None;
                    MAX_SIGNATURES_PER_STATUS_CHECK
                ])));
        }

        finalize_transactions(runtime.clone()).await;

        assert_eq!(
            called_methods(&runtime)
                .iter()
                .filter(|method| *method == "getSignatureStatuses")
                .count(),
            MAX_CONCURRENT_RPC_CALLS
        );
        assert_eq!(
            runtime.set_timer_delays(),
            vec![FINALIZE_TRANSACTIONS_RETRY_DELAY]
        );
    }

    #[tokio::test]
    async fn should_finalize_transaction_with_finalized_status() {
        setup();

        let signature = submit_sweep_transaction(CURRENT_BLOCK_HEIGHT);

        let runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .add_stub_response(SlotResult::Consistent(Ok(CURRENT_SLOT)))
            .add_stub_response(BlockResult::Consistent(Ok(current_block())))
            .add_stub_response(SignatureStatusesResult::Consistent(Ok(vec![Some(
                finalized_status(),
            )])))
            .add_stub_response(GetTransactionResult::Consistent(Ok(None)));

        finalize_transactions(runtime).await;

        EventsAssert::from_recorded()
            .expect_contains_event_eq(EventType::SucceededTransaction { signature });

        read_state(|s| {
            assert!(s.submitted_transactions().is_empty());
            assert!(s.succeeded_transactions().contains(&signature));
        });
    }

    #[tokio::test]
    async fn should_not_finalize_transaction() {
        for status in [processed_status(), confirmed_status()] {
            for block_height in [CURRENT_BLOCK_HEIGHT, EXPIRED_BLOCK_HEIGHT] {
                should_not_finalize(block_height, Some(status.clone())).await;
            }
        }
        should_not_finalize(CURRENT_BLOCK_HEIGHT, None).await;
    }

    async fn should_not_finalize(block_height: BlockHeight, status: Option<TransactionStatus>) {
        reset_state();
        reset_events();
        setup();

        submit_sweep_transaction(block_height);

        let runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .add_stub_response(SlotResult::Consistent(Ok(CURRENT_SLOT)))
            .add_stub_response(BlockResult::Consistent(Ok(current_block())))
            .add_stub_response(SignatureStatusesResult::Consistent(Ok(vec![status])));

        let events_before = EventsAssert::from_recorded();

        finalize_transactions(runtime).await;

        let events_after = EventsAssert::from_recorded();
        assert_eq!(events_before, events_after);

        read_state(|s| assert_eq!(s.submitted_transactions().len(), 1));
    }

    #[tokio::test]
    async fn should_record_failed_transaction_event_on_error() {
        setup();

        let signature = submit_sweep_transaction(CURRENT_BLOCK_HEIGHT);

        let runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .add_stub_response(SlotResult::Consistent(Ok(CURRENT_SLOT)))
            .add_stub_response(BlockResult::Consistent(Ok(current_block())))
            .add_stub_response(SignatureStatusesResult::Consistent(Ok(vec![Some(
                TransactionStatus {
                    slot: SUBMISSION_SLOT,
                    status: Err(TransactionError::InsufficientFundsForFee),
                    err: Some(TransactionError::InsufficientFundsForFee),
                    confirmation_status: Some(TransactionConfirmationStatus::Finalized),
                },
            )])));

        finalize_transactions(runtime).await;

        EventsAssert::from_recorded()
            .expect_contains_event_eq(EventType::FailedTransaction { signature });

        read_state(|s| {
            assert!(s.submitted_transactions().is_empty());
            assert_eq!(s.failed_transactions().len(), 1);
            assert!(s.failed_transactions().contains_key(&signature));
        });
    }

    #[tokio::test]
    async fn should_finalize_multiple_transactions_in_one_batch() {
        setup();

        let sig_a = 0x01;
        let sig_b = 0x02;
        let sig_c = 0x03;
        submit_sweep_transaction_with_signature(sig_a, CURRENT_BLOCK_HEIGHT);
        submit_sweep_transaction_with_signature(sig_b, CURRENT_BLOCK_HEIGHT);
        submit_sweep_transaction_with_signature(sig_c, CURRENT_BLOCK_HEIGHT);

        let runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .add_stub_response(SlotResult::Consistent(Ok(CURRENT_SLOT)))
            .add_stub_response(BlockResult::Consistent(Ok(current_block())))
            .add_stub_response(SignatureStatusesResult::Consistent(Ok(vec![
                Some(finalized_status()),
                None,
                Some(finalized_status()),
            ])))
            .add_stub_response(GetTransactionResult::Consistent(Ok(None)))
            .add_stub_response(GetTransactionResult::Consistent(Ok(None)));

        finalize_transactions(runtime).await;

        EventsAssert::from_recorded()
            .expect_contains_event_eq(EventType::SucceededTransaction {
                signature: signature(sig_a),
            })
            .expect_contains_event_eq(EventType::SucceededTransaction {
                signature: signature(sig_c),
            });

        read_state(|s| {
            assert_eq!(s.submitted_transactions().len(), 1);
            assert!(s.submitted_transactions().contains_key(&signature(sig_b)));
        });
    }

    #[tokio::test]
    async fn should_never_expire_nonce_withdrawal_with_unchanged_nonce() {
        setup();
        let sweep = submit_sweep_transaction_with_signature(1, EXPIRED_BLOCK_HEIGHT);
        let nonce_withdrawal = submit_withdrawal_bound_to(2, BOUND_NONCE_SEED);

        let runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .add_stub_response(SlotResult::Consistent(Ok(CURRENT_SLOT)))
            .add_stub_response(BlockResult::Consistent(Ok(current_block())))
            .add_stub_response(SignatureStatusesResult::Consistent(Ok(vec![None])))
            .add_stub_response(nonce_read(BOUND_NONCE_SEED));

        finalize_transactions(runtime).await;

        let events = EventsAssert::from_recorded()
            .expect_contains_event_eq(EventType::ExpiredTransaction { signature: sweep });
        assert!(!events.contains_event(&EventType::ExpiredTransaction {
            signature: nonce_withdrawal
        }));
        read_state(|s| assert!(s.submitted_transactions().contains_key(&nonce_withdrawal)));
    }

    #[tokio::test]
    async fn should_not_expire_transaction_if_status_check_fails() {
        setup();

        submit_sweep_transaction(EXPIRED_BLOCK_HEIGHT);

        let events_before = EventsAssert::from_recorded();

        let finalize_runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .add_stub_response(SlotResult::Consistent(Ok(CURRENT_SLOT)))
            .add_stub_response(BlockResult::Consistent(Ok(current_block())))
            .add_stub_response(SignatureStatusesResult::Consistent(Err(
                RpcError::ValidationError("Error".to_string()),
            )));

        finalize_transactions(finalize_runtime).await;

        let events_after = EventsAssert::from_recorded();
        assert_eq!(events_before, events_after);

        read_state(|s| {
            assert_eq!(s.submitted_transactions().len(), 1);
        });
    }

    struct ExpiryCase {
        name: &'static str,
        transaction_block_height: BlockHeight,
        should_expire: bool,
    }

    #[tokio::test]
    async fn should_expire_transaction_past_the_blockhash_age_limit() {
        let cases = [
            ExpiryCase {
                name: "at the age limit",
                transaction_block_height: OLDEST_VALID_BLOCK_HEIGHT,
                should_expire: false,
            },
            ExpiryCase {
                name: "past the age limit",
                transaction_block_height: BlockHeight::new(OLDEST_VALID_BLOCK_HEIGHT.get() - 1),
                should_expire: true,
            },
            ExpiryCase {
                name: "ahead of the current block height",
                transaction_block_height: BlockHeight::new(u64::MAX),
                should_expire: false,
            },
        ];

        for case in cases {
            reset_state();
            reset_events();
            setup();
            let signature = submit_sweep_transaction(case.transaction_block_height);
            let runtime = TestCanisterRuntime::new()
                .with_increasing_time()
                .add_stub_response(SlotResult::Consistent(Ok(CURRENT_SLOT)))
                .add_stub_response(BlockResult::Consistent(Ok(current_block())))
                .add_stub_response(SignatureStatusesResult::Consistent(Ok(vec![None])));

            finalize_transactions(runtime).await;

            let expired = EventsAssert::from_recorded()
                .contains_event(&EventType::ExpiredTransaction { signature });
            assert_eq!(expired, case.should_expire, "{}", case.name);
            read_state(|s| {
                assert_eq!(
                    s.submitted_transactions().contains_key(&signature),
                    !case.should_expire,
                    "{}",
                    case.name
                );
            });
        }
    }

    fn confirmed_status() -> TransactionStatus {
        TransactionStatus {
            slot: 0,
            status: Ok(()),
            err: None,
            confirmation_status: Some(TransactionConfirmationStatus::Confirmed),
        }
    }

    fn processed_status() -> TransactionStatus {
        TransactionStatus {
            slot: 0,
            status: Ok(()),
            err: None,
            confirmation_status: Some(TransactionConfirmationStatus::Processed),
        }
    }
}

mod withdrawal_finalization {
    use super::*;

    #[tokio::test]
    async fn should_record_a_withdrawal_as_succeeded_once_its_nonce_advanced() {
        setup();
        let signature = submit_withdrawal_bound_to(1, BOUND_NONCE_SEED);

        let runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .add_stub_response(nonce_read(UNSEEN_NONCE_SEED))
            .add_stub_response(succeeded_withdrawal_response(&signature));

        finalize_transactions(runtime.clone()).await;

        EventsAssert::from_recorded()
            .expect_contains_event_eq(EventType::SucceededTransaction { signature });
        read_state(|s| {
            assert!(s.submitted_transactions().is_empty());
            assert!(s.succeeded_transactions().contains(&signature));
        });
        assert_nonce_account_is_free();
        assert_eq!(
            called_methods(&runtime),
            vec!["getAccountInfo", "getTransaction"]
        );
    }

    #[tokio::test]
    async fn should_record_a_withdrawal_as_failed_once_its_nonce_advanced_with_an_error() {
        setup();
        let signature = submit_withdrawal_bound_to(1, BOUND_NONCE_SEED);

        let runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .add_stub_response(nonce_read(UNSEEN_NONCE_SEED))
            .add_stub_response(failed_withdrawal_response(&signature));

        finalize_transactions(runtime).await;

        EventsAssert::from_recorded()
            .expect_contains_event_eq(EventType::FailedTransaction { signature });
        read_state(|s| {
            assert!(s.submitted_transactions().is_empty());
            assert!(s.failed_transactions().contains_key(&signature));
        });
        assert_nonce_account_is_free();
    }

    #[tokio::test]
    async fn should_rebroadcast_a_withdrawal_with_an_unchanged_nonce_unchanged() {
        setup();
        let signature = submit_withdrawal_bound_to(1, BOUND_NONCE_SEED);
        let events_before = EventsAssert::from_recorded();

        let runtime = TestCanisterRuntime::new()
            .with_increasing_time_from(min_rebroadcast_age_nanos())
            .add_stub_response(nonce_read(BOUND_NONCE_SEED))
            .add_stub_response(SendTransactionResult::Consistent(Ok(signature.into())));

        finalize_transactions(runtime.clone()).await;

        let sent = runtime.sent_transactions();
        assert_eq!(sent.len(), 1);
        assert_eq!(
            sent[0].get_transaction(),
            encoded_submitted_transaction(&signature)
        );
        assert_eq!(sent[0].skip_preflight, Some(true));
        assert_eq!(EventsAssert::from_recorded(), events_before);
        read_state(|s| assert!(s.submitted_transactions().contains_key(&signature)));
    }

    #[tokio::test]
    async fn should_rebroadcast_a_withdrawal_with_an_unchanged_nonce_only_after_the_minimum_age() {
        setup();
        let signature = submit_withdrawal_bound_to(1, BOUND_NONCE_SEED);

        let too_early = TestCanisterRuntime::new()
            .with_increasing_time()
            .add_stub_response(nonce_read(BOUND_NONCE_SEED));

        finalize_transactions(too_early.clone()).await;

        assert!(too_early.sent_transactions().is_empty());

        let old_enough = TestCanisterRuntime::new()
            .with_increasing_time_from(min_rebroadcast_age_nanos())
            .add_stub_response(nonce_read(BOUND_NONCE_SEED))
            .add_stub_response(SendTransactionResult::Consistent(Ok(signature.into())));

        finalize_transactions(old_enough.clone()).await;

        assert_eq!(old_enough.sent_transactions().len(), 1);
        read_state(|s| assert!(s.submitted_transactions().contains_key(&signature)));
    }

    #[tokio::test]
    async fn should_take_no_decision_on_a_stale_nonce_read() {
        setup();
        let landed = submit_withdrawal_bound_to(1, STALE_NONCE_SEED);
        events::succeed_transaction(landed);
        let signature = submit_withdrawal_bound_to(2, BOUND_NONCE_SEED);
        let events_before = EventsAssert::from_recorded();

        let runtime = TestCanisterRuntime::new()
            .with_increasing_time_from(min_rebroadcast_age_nanos())
            .add_stub_response(nonce_read(STALE_NONCE_SEED));

        finalize_transactions(runtime.clone()).await;

        assert_eq!(EventsAssert::from_recorded(), events_before);
        assert!(runtime.sent_transactions().is_empty());
        read_state(|s| assert!(s.submitted_transactions().contains_key(&signature)));
    }

    #[tokio::test]
    async fn should_keep_the_nonce_account_bound_until_the_outcome_is_known() {
        setup();
        let signature = submit_withdrawal_bound_to(1, BOUND_NONCE_SEED);
        let events_before = EventsAssert::from_recorded();

        let runtime = TestCanisterRuntime::new()
            .with_increasing_time_from(min_rebroadcast_age_nanos())
            .add_stub_response(nonce_read(UNSEEN_NONCE_SEED))
            .add_stub_response(GetTransactionResult::Consistent(Ok(None)));

        finalize_transactions(runtime.clone()).await;

        assert_eq!(EventsAssert::from_recorded(), events_before);
        assert!(runtime.sent_transactions().is_empty());
        read_state(|s| {
            assert!(s.submitted_transactions().contains_key(&signature));
            assert_eq!(s.nonce_pool().num_free_accounts(), 0);
        });
        assert_eq!(
            with_unstable_metrics(|m| m.withdrawal_transactions_with_unresolved_outcome),
            1
        );
    }

    fn assert_nonce_account_is_free() {
        read_state(|s| {
            assert!(
                s.nonce_pool()
                    .free_accounts()
                    .any(|account| *account == NONCE_ACCOUNT)
            )
        });
    }
}

const BOUND_NONCE_SEED: usize = 1;
const STALE_NONCE_SEED: usize = 2;
const UNSEEN_NONCE_SEED: usize = 3;

fn min_rebroadcast_age_nanos() -> u64 {
    MIN_REBROADCAST_AGE.as_nanos() as u64
}

fn encoded_submitted_transaction(signature: &solana_signature::Signature) -> String {
    let message = read_state(|s| {
        match s
            .submitted_transactions()
            .get(signature)
            .expect("the transaction is submitted")
        {
            MinterTransaction::Withdrawal {
                message: VersionedMessage::Legacy(message),
                ..
            } => message.clone(),
            other => panic!("expected a withdrawal transaction, got {other:?}"),
        }
    });
    let transaction = Transaction {
        signatures: vec![*signature],
        message,
    };
    SendTransactionParams::try_from(transaction)
        .expect("the transaction is serializable")
        .get_transaction()
        .to_string()
}

/// Submits a withdrawal transaction under `signature(i)` bound to [`NONCE_ACCOUNT`]
/// and to the nonce value that [`nonce_read`] returns for `nonce_seed`.
fn submit_withdrawal_bound_to(i: usize, nonce_seed: usize) -> solana_signature::Signature {
    let signature = signature(i);
    events::accept_withdrawal(account(i), i as u64, MINIMUM_WITHDRAWAL_AMOUNT);
    events::create_withdrawal_batch_transaction(durable_nonce(nonce_seed), vec![i as u64]);
    events::submit_withdrawal_batch_transaction(
        signature,
        durable_nonce(nonce_seed),
        vec![i as u64],
    );
    signature
}

fn nonce_read(nonce_seed: usize) -> GetAccountInfoResult {
    GetAccountInfoResult::Consistent(Ok(Some(nonce_account_info(MINTER_ADDRESS, nonce_seed))))
}

fn called_methods(runtime: &TestCanisterRuntime) -> Vec<String> {
    runtime
        .sent_update_calls()
        .into_iter()
        .map(|call| call.method)
        .collect()
}

fn setup() {
    init_state();
    init_balance();
    init_schnorr_master_key();
}

fn current_block() -> ConfirmedBlock {
    confirmed_block_at_height(CURRENT_BLOCK_HEIGHT)
}

fn submit_sweep_transaction(block_height: BlockHeight) -> solana_signature::Signature {
    submit_sweep_transaction_with_signature(1, block_height)
}

fn submit_sweep_transaction_with_signature(
    i: usize,
    block_height: BlockHeight,
) -> solana_signature::Signature {
    let signature = signature(i);
    let deposit_id = crate::state::read_state(|state| state.deposits().next_id());
    events::queue_deposit(deposit_id, account(i), 1_000_000);
    events::submit_sweep_at_height(signature, vec![deposit_id], block_height);
    signature
}
