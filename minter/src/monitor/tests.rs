use super::{MAX_BLOCKHASH_AGE, monitor_submitted_transactions};
use crate::{
    state::{TaskType, event::EventType, mutate_state, read_state, reset_state},
    storage::reset_events,
    test_fixtures::{
        EventsAssert, MINTER_ACCOUNT, confirmed_block, deposit_id, events, init_schnorr_master_key,
        init_state, runtime::TestCanisterRuntime, signature,
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
const RECENT_SLOT: Slot = CURRENT_SLOT - 10;
const EXPIRED_SLOT: Slot = CURRENT_SLOT - MAX_BLOCKHASH_AGE - 1;
const RESUBMISSION_SLOT: Slot = CURRENT_SLOT + 5;

#[tokio::test]
async fn should_return_early_if_no_submitted_transactions() {
    setup();

    monitor_submitted_transactions(TestCanisterRuntime::new().with_increasing_time()).await;

    EventsAssert::assert_no_events_recorded();
}

#[tokio::test]
async fn should_return_early_if_task_already_active() {
    setup();
    submit_consolidation_transaction(CURRENT_SLOT);

    mutate_state(|s| {
        s.active_tasks_mut()
            .insert(TaskType::MonitorSubmittedTransactions);
    });

    let events_before = EventsAssert::from_recorded();

    // We return early, therefore no RPC calls are made
    let runtime = TestCanisterRuntime::new();

    monitor_submitted_transactions(runtime).await;

    let events_after = EventsAssert::from_recorded();
    assert_eq!(events_before, events_after);
}

#[tokio::test]
async fn should_return_early_if_fetching_current_slot_fails() {
    setup();
    submit_consolidation_transaction(EXPIRED_SLOT);

    let events_before = EventsAssert::from_recorded();

    let error = SlotResult::Consistent(Err(RpcError::ValidationError("Error".to_string())));
    let runtime = TestCanisterRuntime::new()
        .add_stub_response(error.clone())
        .add_stub_response(error.clone())
        .add_stub_response(error);

    monitor_submitted_transactions(runtime).await;

    let events_after = EventsAssert::from_recorded();
    assert_eq!(events_before, events_after);
}

mod finalization {
    use super::*;

    #[tokio::test]
    async fn should_finalize_transaction_with_finalized_status() {
        setup();

        let signature = submit_consolidation_transaction(RECENT_SLOT);

        let runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .add_stub_response(SlotResult::Consistent(Ok(CURRENT_SLOT)))
            .add_stub_response(BlockResult::Consistent(Ok(confirmed_block())))
            .add_stub_response(SignatureStatusesResult::Consistent(Ok(vec![Some(
                finalized_status(),
            )])));

        monitor_submitted_transactions(runtime).await;

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
            .add_stub_response(SlotResult::Consistent(Ok(CURRENT_SLOT)))
            .add_stub_response(BlockResult::Consistent(Ok(confirmed_block())))
            .add_stub_response(SignatureStatusesResult::Consistent(Ok(vec![
                status.clone(),
            ])));

        let _ = status; // suppress unused warning

        let events_before = EventsAssert::from_recorded();

        monitor_submitted_transactions(runtime).await;

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
            .add_stub_response(SlotResult::Consistent(Ok(CURRENT_SLOT)))
            .add_stub_response(BlockResult::Consistent(Ok(confirmed_block())))
            .add_stub_response(SignatureStatusesResult::Consistent(Ok(vec![Some(
                TransactionStatus {
                    slot: RECENT_SLOT,
                    status: Err(TransactionError::InsufficientFundsForFee),
                    err: Some(TransactionError::InsufficientFundsForFee),
                    confirmation_status: Some(TransactionConfirmationStatus::Finalized),
                },
            )])));

        monitor_submitted_transactions(runtime).await;

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
            .add_stub_response(SlotResult::Consistent(Ok(CURRENT_SLOT)))
            .add_stub_response(BlockResult::Consistent(Ok(confirmed_block())))
            .add_stub_response(SignatureStatusesResult::Consistent(Ok(vec![
                Some(finalized_status()),
                None,
                Some(finalized_status()),
            ])));
        // sig_b is not_found but RECENT_SLOT is not expired, so no resubmission.

        monitor_submitted_transactions(runtime).await;

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
    async fn should_resubmit_expired_transaction_with_no_status() {
        setup();

        let old_signature = submit_consolidation_transaction(EXPIRED_SLOT);

        let new_signature = signature(0xAA);

        let runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .add_stub_response(SlotResult::Consistent(Ok(CURRENT_SLOT)))
            .add_stub_response(BlockResult::Consistent(Ok(confirmed_block())))
            .add_stub_response(SignatureStatusesResult::Consistent(Ok(vec![None])))
            .add_stub_response(SlotResult::Consistent(Ok(RESUBMISSION_SLOT)))
            .add_stub_response(BlockResult::Consistent(Ok(confirmed_block())))
            .add_stub_response(SendTransactionResult::Consistent(Ok(new_signature.into())))
            .add_signature(new_signature.into());

        monitor_submitted_transactions(runtime).await;

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

        let runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .add_stub_response(SlotResult::Consistent(Ok(CURRENT_SLOT)))
            .add_stub_response(BlockResult::Consistent(Ok(confirmed_block())))
            .add_stub_response(SignatureStatusesResult::Consistent(Err(
                RpcError::ValidationError("Error".to_string()),
            )));

        monitor_submitted_transactions(runtime).await;

        let events_after = EventsAssert::from_recorded();
        assert_eq!(events_before, events_after);

        read_state(|s| assert_eq!(s.submitted_transactions().len(), 1));
    }

    #[tokio::test]
    async fn should_record_resubmission_event_even_if_submission_fails() {
        setup();

        let old_signature = submit_consolidation_transaction(EXPIRED_SLOT);

        let new_signature = signature(0xAA);

        let runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .add_stub_response(SlotResult::Consistent(Ok(CURRENT_SLOT)))
            .add_stub_response(BlockResult::Consistent(Ok(confirmed_block())))
            .add_stub_response(SignatureStatusesResult::Consistent(Ok(vec![None])))
            .add_stub_response(SlotResult::Consistent(Ok(RESUBMISSION_SLOT)))
            .add_stub_response(BlockResult::Consistent(Ok(confirmed_block())))
            .add_stub_response(SendTransactionResult::Inconsistent(vec![]))
            .add_signature(new_signature.into());

        monitor_submitted_transactions(runtime).await;

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
    async fn should_resubmit_multiple_expired_transactions_in_batches() {
        use crate::constants::MAX_CONCURRENT_RPC_CALLS;

        setup();

        let num_transactions = MAX_CONCURRENT_RPC_CALLS + 2; // 10+2 = 12 transactions require 2 rounds
        for i in 0..num_transactions {
            submit_consolidation_transaction_with_signature(i as u8, EXPIRED_SLOT);
        }

        let mut runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .add_stub_response(SlotResult::Consistent(Ok(CURRENT_SLOT)))
            .add_stub_response(BlockResult::Consistent(Ok(confirmed_block())))
            .add_stub_response(SignatureStatusesResult::Consistent(Ok(
                vec![None; num_transactions],
            )))
            .add_stub_response(SlotResult::Consistent(Ok(RESUBMISSION_SLOT)))
            .add_stub_response(BlockResult::Consistent(Ok(confirmed_block())));

        for i in 0..MAX_CONCURRENT_RPC_CALLS {
            runtime = runtime
                .add_stub_response(SendTransactionResult::Consistent(Ok(signature(
                    0xA0 + i as u8,
                )
                .into())))
                .add_signature([0xA0 + i as u8; 64]);
        }

        runtime = runtime
            .add_stub_response(SlotResult::Consistent(Ok(RESUBMISSION_SLOT)))
            .add_stub_response(BlockResult::Consistent(Ok(confirmed_block())));

        for i in 0..2_usize {
            runtime = runtime
                .add_stub_response(SendTransactionResult::Consistent(Ok(signature(
                    0xB0 + i as u8,
                )
                .into())))
                .add_signature([0xB0 + i as u8; 64]);
        }

        monitor_submitted_transactions(runtime).await;

        read_state(|s| assert_eq!(s.submitted_transactions().len(), num_transactions));
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
    i: u8,
    slot: Slot,
) -> solana_signature::Signature {
    let signature = signature(i);
    events::accept_deposit(deposit_id(i), 1_000_000);
    events::mint_deposit(deposit_id(i), i as u64);
    events::submit_consolidation(signature, MINTER_ACCOUNT, slot, vec![i as u64]);
    signature
}
