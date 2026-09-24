use super::{
    MAX_BLOCKHASH_AGE_IN_BLOCKS, MAX_SIGNATURES_PER_STATUS_CHECK, finalize_transactions,
    resubmit_transactions,
};
use crate::{
    constants::MAX_CONCURRENT_RPC_CALLS,
    state::{TaskType, event::EventType, mutate_state, read_state, reset_state},
    storage::reset_events,
    test_fixtures::{
        EventsAssert, MINTER_ACCOUNT, confirmed_block_at_height, deposit_id, events,
        init_schnorr_master_key, init_state, runtime::TestCanisterRuntime, signature,
    },
};
use sol_rpc_types::{
    ConfirmedBlock, MultiRpcResult, RpcError, Signature, Slot, TransactionConfirmationStatus,
    TransactionError, TransactionStatus,
};

type SlotResult = MultiRpcResult<Slot>;
type BlockResult = MultiRpcResult<ConfirmedBlock>;
type SendTransactionResult = MultiRpcResult<Signature>;
type SignatureStatusesResult = MultiRpcResult<Vec<Option<TransactionStatus>>>;

const CURRENT_SLOT: Slot = 408_807_102;
const SUBMISSION_SLOT: Slot = CURRENT_SLOT - 10;
const CURRENT_BLOCK_HEIGHT: u64 = CURRENT_SLOT - 1_000;
const OLDEST_VALID_BLOCK_HEIGHT: u64 = CURRENT_BLOCK_HEIGHT - MAX_BLOCKHASH_AGE_IN_BLOCKS;
const EXPIRED_BLOCK_HEIGHT: u64 = OLDEST_VALID_BLOCK_HEIGHT - 1;
const RESUBMISSION_SLOT: Slot = CURRENT_SLOT + 5;
const RESUBMISSION_BLOCK_HEIGHT: u64 = RESUBMISSION_SLOT - 1_000;

mod finalization {
    use super::*;

    #[tokio::test]
    async fn should_return_early_if_no_submitted_transactions() {
        setup();

        finalize_transactions(TestCanisterRuntime::new().with_increasing_time()).await;

        EventsAssert::assert_no_events_recorded();
    }

    #[tokio::test]
    async fn should_return_early_if_task_already_active() {
        setup();
        submit_consolidation_transaction(CURRENT_BLOCK_HEIGHT);

        mutate_state(|s| {
            s.active_tasks_mut().insert(TaskType::FinalizeTransactions);
        });

        let events_before = EventsAssert::from_recorded();

        finalize_transactions(TestCanisterRuntime::new()).await;

        let events_after = EventsAssert::from_recorded();
        assert_eq!(events_before, events_after);
    }

    #[tokio::test]
    async fn should_return_early_if_fetching_current_block_fails() {
        setup();
        submit_consolidation_transaction(EXPIRED_BLOCK_HEIGHT);

        let events_before = EventsAssert::from_recorded();

        let error = SlotResult::Consistent(Err(RpcError::ValidationError("Error".to_string())));
        let runtime = TestCanisterRuntime::new()
            .add_stub_response(error.clone())
            .add_stub_response(error.clone())
            .add_stub_response(error);

        finalize_transactions(runtime).await;

        let events_after = EventsAssert::from_recorded();
        assert_eq!(events_before, events_after);
    }

    #[tokio::test]
    async fn should_reschedule_until_all_transactions_finalized() {
        setup();

        let num = MAX_CONCURRENT_RPC_CALLS * MAX_SIGNATURES_PER_STATUS_CHECK + 1;
        for i in 0..num {
            submit_consolidation_transaction_with_signature(i, CURRENT_BLOCK_HEIGHT);
        }

        // Round 1: finalizes MAX_CONCURRENT_RPC_CALLS batches, 1 transaction unchecked → reschedule
        let mut runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .add_stub_response(SlotResult::Consistent(Ok(CURRENT_SLOT)))
            .add_stub_response(BlockResult::Consistent(Ok(current_block())));
        for _ in 0..MAX_CONCURRENT_RPC_CALLS {
            runtime = runtime.add_stub_response(SignatureStatusesResult::Consistent(Ok(
                vec![Some(finalized_status()); MAX_SIGNATURES_PER_STATUS_CHECK],
            )));
        }

        finalize_transactions(runtime.clone()).await;

        assert_eq!(read_state(|s| s.submitted_transactions().len()), 1);
        assert_eq!(runtime.set_timer_call_count(), 1);

        // Round 2: finalizes the remaining 1 transaction → no reschedule
        let runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .add_stub_response(SlotResult::Consistent(Ok(CURRENT_SLOT)))
            .add_stub_response(BlockResult::Consistent(Ok(current_block())))
            .add_stub_response(SignatureStatusesResult::Consistent(Ok(vec![Some(
                finalized_status(),
            )])));

        finalize_transactions(runtime.clone()).await;

        assert!(read_state(|s| s.submitted_transactions().is_empty()));
        assert_eq!(runtime.set_timer_call_count(), 0);
    }

    #[tokio::test]
    async fn should_finalize_transaction_with_finalized_status() {
        setup();

        let signature = submit_consolidation_transaction(CURRENT_BLOCK_HEIGHT);

        let runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .add_stub_response(SlotResult::Consistent(Ok(CURRENT_SLOT)))
            .add_stub_response(BlockResult::Consistent(Ok(current_block())))
            .add_stub_response(SignatureStatusesResult::Consistent(Ok(vec![Some(
                finalized_status(),
            )])));

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

    async fn should_not_finalize(block_height: u64, status: Option<TransactionStatus>) {
        reset_state();
        reset_events();
        setup();

        submit_consolidation_transaction(block_height);

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

        let signature = submit_consolidation_transaction(CURRENT_BLOCK_HEIGHT);

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
        submit_consolidation_transaction_with_signature(sig_a, CURRENT_BLOCK_HEIGHT);
        submit_consolidation_transaction_with_signature(sig_b, CURRENT_BLOCK_HEIGHT);
        submit_consolidation_transaction_with_signature(sig_c, CURRENT_BLOCK_HEIGHT);

        let runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .add_stub_response(SlotResult::Consistent(Ok(CURRENT_SLOT)))
            .add_stub_response(BlockResult::Consistent(Ok(current_block())))
            .add_stub_response(SignatureStatusesResult::Consistent(Ok(vec![
                Some(finalized_status()),
                None,
                Some(finalized_status()),
            ])));

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
            assert!(s.transactions_to_resubmit().is_empty());
        });
    }

    struct ExpiryCase {
        name: &'static str,
        transaction_block_height: u64,
        should_expire: bool,
    }

    #[tokio::test]
    async fn should_expire_transaction_past_the_blockhash_age_limit() {
        let cases = [
            ExpiryCase {
                name: "below the age limit",
                transaction_block_height: OLDEST_VALID_BLOCK_HEIGHT + 1,
                should_expire: false,
            },
            ExpiryCase {
                name: "at the age limit",
                transaction_block_height: OLDEST_VALID_BLOCK_HEIGHT,
                should_expire: false,
            },
            ExpiryCase {
                name: "past the age limit",
                transaction_block_height: OLDEST_VALID_BLOCK_HEIGHT - 1,
                should_expire: true,
            },
            ExpiryCase {
                name: "ahead of the current block height",
                transaction_block_height: u64::MAX,
                should_expire: false,
            },
        ];

        for case in cases {
            reset_state();
            reset_events();
            setup();
            let signature = submit_consolidation_transaction(case.transaction_block_height);
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
                    s.transactions_to_resubmit().contains_key(&signature),
                    case.should_expire,
                    "{}",
                    case.name
                );
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

    fn finalized_status() -> TransactionStatus {
        TransactionStatus {
            slot: 0,
            status: Ok(()),
            err: None,
            confirmation_status: Some(TransactionConfirmationStatus::Finalized),
        }
    }
}

mod resubmission {
    use super::*;

    #[tokio::test]
    async fn should_return_early_if_no_transactions_to_resubmit() {
        setup();

        resubmit_transactions(TestCanisterRuntime::new().with_increasing_time()).await;

        EventsAssert::assert_no_events_recorded();
    }

    #[tokio::test]
    async fn should_return_early_if_task_already_active() {
        setup();
        let sig = submit_consolidation_transaction(EXPIRED_BLOCK_HEIGHT);
        events::expire_transaction(sig);

        mutate_state(|s| {
            s.active_tasks_mut().insert(TaskType::ResubmitTransactions);
        });

        let events_before = EventsAssert::from_recorded();

        resubmit_transactions(TestCanisterRuntime::new()).await;

        let events_after = EventsAssert::from_recorded();
        assert_eq!(events_before, events_after);
    }

    #[tokio::test]
    async fn should_resubmit_expired_transaction_with_no_status() {
        setup();

        let old_signature = submit_consolidation_transaction(EXPIRED_BLOCK_HEIGHT);
        let new_signature = signature(0xAA);
        events::expire_transaction(old_signature);

        read_state(|s| {
            assert!(s.transactions_to_resubmit().contains_key(&old_signature));
        });

        let resubmit_runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .add_stub_response(SlotResult::Consistent(Ok(RESUBMISSION_SLOT)))
            .add_stub_response(BlockResult::Consistent(Ok(confirmed_block_at_height(
                RESUBMISSION_BLOCK_HEIGHT,
            ))))
            .add_stub_response(SendTransactionResult::Consistent(Ok(new_signature.into())))
            .add_signature(new_signature.into());

        resubmit_transactions(resubmit_runtime).await;

        EventsAssert::from_recorded()
            .expect_contains_event_eq(EventType::ExpiredTransaction {
                signature: old_signature,
            })
            .expect_contains_event_eq(EventType::ResubmittedTransaction {
                old_signature,
                new_signature,
                new_slot: RESUBMISSION_SLOT,
                new_block_height: RESUBMISSION_BLOCK_HEIGHT,
            });

        read_state(|s| {
            assert_eq!(s.submitted_transactions().len(), 1);
            let resubmitted = s.submitted_transactions().get(&new_signature).unwrap();
            assert_eq!(resubmitted.slot, RESUBMISSION_SLOT);
            assert_eq!(resubmitted.block_height, RESUBMISSION_BLOCK_HEIGHT);
        });
    }

    #[tokio::test]
    async fn should_not_resubmit_expired_transaction_if_status_check_fails() {
        setup();

        submit_consolidation_transaction(EXPIRED_BLOCK_HEIGHT);

        let events_before = EventsAssert::from_recorded();

        let finalize_runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .add_stub_response(SlotResult::Consistent(Ok(CURRENT_SLOT)))
            .add_stub_response(BlockResult::Consistent(Ok(confirmed_block_at_height(
                RESUBMISSION_BLOCK_HEIGHT,
            ))))
            .add_stub_response(SignatureStatusesResult::Consistent(Err(
                RpcError::ValidationError("Error".to_string()),
            )));

        finalize_transactions(finalize_runtime).await;

        let events_after = EventsAssert::from_recorded();
        assert_eq!(events_before, events_after);

        read_state(|s| {
            assert_eq!(s.submitted_transactions().len(), 1);
            assert!(s.transactions_to_resubmit().is_empty());
        });
    }

    #[tokio::test]
    async fn should_record_resubmission_event_even_if_submission_fails() {
        setup();

        let old_signature = submit_consolidation_transaction(EXPIRED_BLOCK_HEIGHT);
        let new_signature = signature(0xAA);
        events::expire_transaction(old_signature);

        let resubmit_runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .add_stub_response(SlotResult::Consistent(Ok(RESUBMISSION_SLOT)))
            .add_stub_response(BlockResult::Consistent(Ok(confirmed_block_at_height(
                RESUBMISSION_BLOCK_HEIGHT,
            ))))
            .add_stub_response(SendTransactionResult::Inconsistent(vec![]))
            .add_signature(new_signature.into());

        resubmit_transactions(resubmit_runtime).await;

        EventsAssert::from_recorded()
            .expect_contains_event_eq(EventType::ExpiredTransaction {
                signature: old_signature,
            })
            .expect_contains_event_eq(EventType::ResubmittedTransaction {
                old_signature,
                new_signature,
                new_slot: RESUBMISSION_SLOT,
                new_block_height: RESUBMISSION_BLOCK_HEIGHT,
            });
    }

    #[tokio::test]
    async fn should_reschedule_until_all_transactions_resubmitted() {
        setup();

        let num_transactions = MAX_CONCURRENT_RPC_CALLS + 1;
        for i in 0..num_transactions {
            let sig = submit_consolidation_transaction_with_signature(i, EXPIRED_BLOCK_HEIGHT);
            events::expire_transaction(sig);
        }

        // Round 1: resubmits MAX_CONCURRENT_RPC_CALLS transactions, 1 remain → reschedule
        let mut runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .add_stub_response(SlotResult::Consistent(Ok(RESUBMISSION_SLOT)))
            .add_stub_response(BlockResult::Consistent(Ok(confirmed_block_at_height(
                RESUBMISSION_BLOCK_HEIGHT,
            ))));
        for i in 0..MAX_CONCURRENT_RPC_CALLS {
            runtime = runtime
                .add_stub_response(SendTransactionResult::Consistent(Ok(
                    signature(0xA0 + i).into()
                )))
                .add_signature(signature(0xA0 + i).into());
        }

        resubmit_transactions(runtime.clone()).await;

        read_state(|s| {
            assert_eq!(s.submitted_transactions().len(), MAX_CONCURRENT_RPC_CALLS);
            assert_eq!(
                s.transactions_to_resubmit().len(),
                num_transactions - MAX_CONCURRENT_RPC_CALLS
            );
        });
        assert_eq!(runtime.set_timer_call_count(), 1);

        // Round 2: resubmits remaining transaction → no reschedule
        let mut runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .add_stub_response(SlotResult::Consistent(Ok(RESUBMISSION_SLOT)))
            .add_stub_response(BlockResult::Consistent(Ok(confirmed_block_at_height(
                RESUBMISSION_BLOCK_HEIGHT,
            ))));
        for i in 0..(num_transactions - MAX_CONCURRENT_RPC_CALLS) {
            runtime = runtime
                .add_stub_response(SendTransactionResult::Consistent(Ok(
                    signature(0xB0 + i).into()
                )))
                .add_signature(signature(0xB0 + i).into());
        }

        resubmit_transactions(runtime.clone()).await;

        assert!(read_state(|s| s.transactions_to_resubmit().is_empty()));
        assert_eq!(runtime.set_timer_call_count(), 0);
    }
}

fn setup() {
    init_state();
    init_schnorr_master_key();
}

fn current_block() -> ConfirmedBlock {
    confirmed_block_at_height(CURRENT_BLOCK_HEIGHT)
}

fn submit_consolidation_transaction(block_height: u64) -> solana_signature::Signature {
    submit_consolidation_transaction_with_signature(1, block_height)
}

fn submit_consolidation_transaction_with_signature(
    i: usize,
    block_height: u64,
) -> solana_signature::Signature {
    let signature = signature(i);
    events::accept_deposit(deposit_id(i), 1_000_000);
    events::mint_deposit(deposit_id(i), i as u64);
    events::submit_consolidation_at_height(
        signature,
        MINTER_ACCOUNT,
        SUBMISSION_SLOT,
        block_height,
        vec![i as u64],
    );
    signature
}
