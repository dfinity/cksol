use super::{
    MAX_BLOCKHASH_AGE, MAX_SIGNATURES_PER_STATUS_CHECK, finalize_transactions,
    resubmit_transactions,
};
use crate::{
    constants::MAX_CONCURRENT_RPC_CALLS,
    state::{TaskType, event::EventType, mutate_state, read_state, reset_state},
    storage::reset_events,
    test_fixtures::{
        EventsAssert, MINTER_ACCOUNT, confirmed_block, deposit_id, events, init_schnorr_master_key,
        init_state, runtime::TestCanisterRuntime, signature,
    },
};
use sol_rpc_types::{
    MultiRpcResult, RpcError, Signature, Slot, TransactionConfirmationStatus, TransactionError,
    TransactionStatus,
};

const CURRENT_SLOT: Slot = 408_807_102;
const RECENT_SLOT: Slot = CURRENT_SLOT - 10;
const EXPIRED_SLOT: Slot = CURRENT_SLOT - MAX_BLOCKHASH_AGE - 1;
const RESUBMISSION_SLOT: Slot = CURRENT_SLOT + 5;

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
        submit_consolidation_transaction(CURRENT_SLOT);

        mutate_state(|s| {
            s.active_tasks_mut().insert(TaskType::FinalizeTransactions);
        });

        let events_before = EventsAssert::from_recorded();

        finalize_transactions(TestCanisterRuntime::new()).await;

        let events_after = EventsAssert::from_recorded();
        assert_eq!(events_before, events_after);
    }

    #[tokio::test]
    async fn should_return_early_if_fetching_current_slot_fails() {
        setup();
        submit_consolidation_transaction(EXPIRED_SLOT);

        let events_before = EventsAssert::from_recorded();

        let runtime = TestCanisterRuntime::new()
            .add_n_get_slot_error(RpcError::ValidationError("Error".to_string()), 3);

        finalize_transactions(runtime).await;

        let events_after = EventsAssert::from_recorded();
        assert_eq!(events_before, events_after);
    }

    #[tokio::test]
    async fn should_reschedule_until_all_transactions_finalized() {
        setup();

        let num = MAX_CONCURRENT_RPC_CALLS * MAX_SIGNATURES_PER_STATUS_CHECK + 1;
        for i in 0..num {
            submit_consolidation_transaction_with_signature(i, RECENT_SLOT);
        }

        // Round 1: finalizes MAX_CONCURRENT_RPC_CALLS batches, 1 transaction unchecked → reschedule
        let mut runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .add_get_slot_response(CURRENT_SLOT)
            .add_get_block_response(confirmed_block());
        for _ in 0..MAX_CONCURRENT_RPC_CALLS {
            runtime =
                runtime.add_get_signature_statuses_response(vec![
                    Some(finalized_status());
                    MAX_SIGNATURES_PER_STATUS_CHECK
                ]);
        }

        finalize_transactions(runtime.clone()).await;

        assert_eq!(read_state(|s| s.submitted_transactions().len()), 1);
        assert_eq!(runtime.set_timer_call_count(), 1);

        // Round 2: finalizes the remaining 1 transaction → no reschedule
        let runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .add_get_slot_response(CURRENT_SLOT)
            .add_get_block_response(confirmed_block())
            .add_get_signature_statuses_response(vec![Some(finalized_status())]);

        finalize_transactions(runtime.clone()).await;

        assert!(read_state(|s| s.submitted_transactions().is_empty()));
        assert_eq!(runtime.set_timer_call_count(), 0);
    }

    #[tokio::test]
    async fn should_finalize_transaction_with_finalized_status() {
        setup();

        let signature = submit_consolidation_transaction(RECENT_SLOT);

        let runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .add_get_slot_response(CURRENT_SLOT)
            .add_get_block_response(confirmed_block())
            .add_get_signature_statuses_response(vec![Some(finalized_status())]);

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
        // Processed status, recent slot
        should_not_finalize(RECENT_SLOT, Some(processed_status())).await;
        // Processed status, expired slot
        should_not_finalize(EXPIRED_SLOT, Some(processed_status())).await;
        // Confirmed status, recent slot
        should_not_finalize(RECENT_SLOT, Some(confirmed_status())).await;
        // Confirmed status, expired slot
        should_not_finalize(EXPIRED_SLOT, Some(confirmed_status())).await;
        // No status, blockhash not yet expired
        should_not_finalize(RECENT_SLOT, None).await;
    }

    async fn should_not_finalize(slot: Slot, status: Option<TransactionStatus>) {
        reset_state();
        reset_events();
        setup();

        submit_consolidation_transaction(slot);

        let runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .add_get_slot_response(CURRENT_SLOT)
            .add_get_block_response(confirmed_block())
            .add_get_signature_statuses_response(vec![status.clone()]);

        let _ = status; // suppress unused warning

        let events_before = EventsAssert::from_recorded();

        finalize_transactions(runtime).await;

        let events_after = EventsAssert::from_recorded();
        assert_eq!(events_before, events_after);

        read_state(|s| assert_eq!(s.submitted_transactions().len(), 1));
    }

    #[tokio::test]
    async fn should_record_failed_transaction_event_on_error() {
        setup();

        let signature = submit_consolidation_transaction(RECENT_SLOT);

        let runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .add_get_slot_response(CURRENT_SLOT)
            .add_get_block_response(confirmed_block())
            .add_get_signature_statuses_response(vec![Some(TransactionStatus {
                slot: RECENT_SLOT,
                status: Err(TransactionError::InsufficientFundsForFee),
                err: Some(TransactionError::InsufficientFundsForFee),
                confirmation_status: Some(TransactionConfirmationStatus::Finalized),
            })]);

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
        submit_consolidation_transaction_with_signature(sig_a, RECENT_SLOT);
        submit_consolidation_transaction_with_signature(sig_b, RECENT_SLOT);
        submit_consolidation_transaction_with_signature(sig_c, RECENT_SLOT);

        let runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .add_get_slot_response(CURRENT_SLOT)
            .add_get_block_response(confirmed_block())
            .add_get_signature_statuses_response(vec![
                Some(finalized_status()),
                None,
                Some(finalized_status()),
            ]);
        // sig_b is not_found but RECENT_SLOT is not expired, so no resubmission.

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
            // sig_b is not yet expired (RECENT_SLOT), so not marked for resubmission
            assert!(s.transactions_to_resubmit().is_empty());
        });
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
        let sig = submit_consolidation_transaction(EXPIRED_SLOT);
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

        let old_signature = submit_consolidation_transaction(EXPIRED_SLOT);
        let new_signature = signature(0xAA);
        events::expire_transaction(old_signature);

        read_state(|s| {
            assert!(s.transactions_to_resubmit().contains_key(&old_signature));
        });

        let resubmit_runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .add_get_slot_response(RESUBMISSION_SLOT)
            .add_get_block_response(confirmed_block())
            .add_send_transaction_response(new_signature)
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
            });

        read_state(|s| {
            assert_eq!(s.submitted_transactions().len(), 1);
            assert!(s.submitted_transactions().contains_key(&new_signature));
        });
    }

    #[tokio::test]
    async fn should_not_resubmit_expired_transaction_if_status_check_fails() {
        setup();

        submit_consolidation_transaction(EXPIRED_SLOT);

        let events_before = EventsAssert::from_recorded();

        let finalize_runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .add_get_slot_response(CURRENT_SLOT)
            .add_get_block_response(confirmed_block())
            .add_get_signature_statuses_error(RpcError::ValidationError("Error".to_string()));

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

        let old_signature = submit_consolidation_transaction(EXPIRED_SLOT);
        let new_signature = signature(0xAA);
        events::expire_transaction(old_signature);

        let resubmit_runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .add_get_slot_response(RESUBMISSION_SLOT)
            .add_get_block_response(confirmed_block())
            .add_stub_response(MultiRpcResult::<Signature>::Inconsistent(vec![]))
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
            });
    }

    #[tokio::test]
    async fn should_reschedule_until_all_transactions_resubmitted() {
        setup();

        let num_transactions = MAX_CONCURRENT_RPC_CALLS + 1;
        for i in 0..num_transactions {
            let sig = submit_consolidation_transaction_with_signature(i, EXPIRED_SLOT);
            events::expire_transaction(sig);
        }

        // Round 1: resubmits MAX_CONCURRENT_RPC_CALLS transactions, 1 remain → reschedule
        let mut runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .add_get_slot_response(RESUBMISSION_SLOT)
            .add_get_block_response(confirmed_block());
        for i in 0..MAX_CONCURRENT_RPC_CALLS {
            runtime = runtime
                .add_send_transaction_response(signature(0xA0 + i))
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
            .add_get_slot_response(RESUBMISSION_SLOT)
            .add_get_block_response(confirmed_block());
        for i in 0..(num_transactions - MAX_CONCURRENT_RPC_CALLS) {
            runtime = runtime
                .add_send_transaction_response(signature(0xB0 + i))
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

fn submit_consolidation_transaction(slot: Slot) -> solana_signature::Signature {
    submit_consolidation_transaction_with_signature(1, slot)
}

fn submit_consolidation_transaction_with_signature(
    i: usize,
    slot: Slot,
) -> solana_signature::Signature {
    let signature = signature(i);
    events::accept_deposit(deposit_id(i), 1_000_000);
    events::mint_deposit(deposit_id(i), i as u64);
    events::submit_consolidation(signature, MINTER_ACCOUNT, slot, vec![i as u64]);
    signature
}
