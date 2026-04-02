use super::{MAX_BLOCKHASH_AGE, monitor_submitted_transactions};
use crate::{
    state::{TaskType, event::EventType, mutate_state, read_state, reset_state},
    storage::reset_events,
    test_fixtures::{
        EventsAssert, MINTER_ACCOUNT, confirmed_block, events, init_schnorr_master_key, init_state,
        runtime::TestCanisterRuntime, signature,
    },
};
use assert_matches::assert_matches;
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
    add_submitted_transaction(signature(0x01), EXPIRED_SLOT);

    mutate_state(|s| {
        s.active_tasks_mut()
            .insert(TaskType::MonitorSubmittedTransactions);
    });

    monitor_submitted_transactions(TestCanisterRuntime::new()).await;

    EventsAssert::from_recorded()
        .expect_event(|e| assert_matches!(e, EventType::SubmittedTransaction { .. }))
        .assert_no_more_events();
}

#[tokio::test]
async fn should_return_early_if_fetching_current_slot_fails() {
    setup();
    add_submitted_transaction(signature(0x01), EXPIRED_SLOT);

    let error = SlotResult::Consistent(Err(RpcError::ValidationError("Error".to_string())));
    let runtime = TestCanisterRuntime::new()
        .add_stub_response(error.clone())
        .add_stub_response(error.clone())
        .add_stub_response(error);

    monitor_submitted_transactions(runtime).await;

    EventsAssert::from_recorded()
        .expect_event(|e| assert_matches!(e, EventType::SubmittedTransaction { .. }))
        .assert_no_more_events();
}

mod finalization {
    use super::*;

    #[tokio::test]
    async fn should_finalize_transaction_with_finalized_status() {
        setup();

        let signature = signature(0x01);
        add_submitted_transaction(signature, RECENT_SLOT);

        let runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .add_stub_response(SignatureStatusesResult::Consistent(Ok(vec![Some(
                finalized_status(RECENT_SLOT),
            )])));

        monitor_submitted_transactions(runtime).await;

        EventsAssert::from_recorded()
            .expect_event(|e| assert_matches!(e, EventType::SubmittedTransaction { .. }))
            .expect_event(|e| {
                assert_matches!(
                    e,
                    EventType::SucceededTransaction { signature: sig } if sig == signature
                )
            })
            .assert_no_more_events();

        read_state(|s| {
            assert!(s.submitted_transactions().is_empty());
            assert!(s.succeeded_transactions().contains(&signature));
        });
    }

    #[tokio::test]
    async fn should_not_finalize_transaction() {
        // Processed status, recent slot
        should_not_finalize(RECENT_SLOT, Some(processed_status(RECENT_SLOT))).await;
        // Processed status, expired slot
        should_not_finalize(EXPIRED_SLOT, Some(processed_status(EXPIRED_SLOT))).await;
        // Confirmed status, recent slot
        should_not_finalize(RECENT_SLOT, Some(confirmed_status(RECENT_SLOT))).await;
        // Confirmed status, expired slot
        should_not_finalize(EXPIRED_SLOT, Some(confirmed_status(EXPIRED_SLOT))).await;
        // No status, blockhash not yet expired
        should_not_finalize(RECENT_SLOT, None).await;
    }

    async fn should_not_finalize(slot: Slot, status: Option<TransactionStatus>) {
        reset_state();
        reset_events();
        setup();

        add_submitted_transaction(signature(0x01), slot);

        let mut runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .add_stub_response(SignatureStatusesResult::Consistent(Ok(vec![
                status.clone(),
            ])));

        if status.is_none() {
            // get_recent_slot_and_blockhash for current slot check (getSlot + getBlock)
            runtime = runtime
                .add_stub_response(SlotResult::Consistent(Ok(CURRENT_SLOT)))
                .add_stub_response(BlockResult::Consistent(Ok(confirmed_block())));
        }

        monitor_submitted_transactions(runtime).await;

        EventsAssert::from_recorded()
            .expect_event(|e| assert_matches!(e, EventType::SubmittedTransaction { .. }))
            .assert_no_more_events();

        read_state(|s| assert_eq!(s.submitted_transactions().len(), 1));
    }

    #[tokio::test]
    async fn should_record_failed_transaction_event_on_error() {
        setup();

        let signature = signature(0x01);
        add_submitted_transaction(signature, RECENT_SLOT);

        let runtime = TestCanisterRuntime::new()
            .with_increasing_time()
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
            .expect_event(|e| assert_matches!(e, EventType::SubmittedTransaction { .. }))
            .expect_event(|e| {
                assert_matches!(
                    e,
                    EventType::FailedTransaction {
                        signature: sig,
                    } if sig == signature
                )
            })
            .assert_no_more_events();

        read_state(|s| {
            assert!(s.submitted_transactions().is_empty());
            assert_eq!(s.failed_transactions().len(), 1);
            assert!(s.failed_transactions().contains_key(&signature));
        });
    }

    #[tokio::test]
    async fn should_finalize_multiple_transactions_in_one_batch() {
        setup();

        let sig_a = signature(0x01);
        let sig_b = signature(0x02);
        let sig_c = signature(0x03);
        add_submitted_transaction(sig_a, RECENT_SLOT);
        add_submitted_transaction(sig_b, RECENT_SLOT);
        add_submitted_transaction(sig_c, RECENT_SLOT);

        let runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .add_stub_response(SignatureStatusesResult::Consistent(Ok(vec![
                Some(finalized_status(RECENT_SLOT)),
                None,
                Some(finalized_status(RECENT_SLOT)),
            ])))
            // get_recent_slot_and_blockhash for the one not_found transaction (getSlot + getBlock)
            .add_stub_response(SlotResult::Consistent(Ok(CURRENT_SLOT)))
            .add_stub_response(BlockResult::Consistent(Ok(confirmed_block())));

        monitor_submitted_transactions(runtime).await;

        EventsAssert::from_recorded()
            .expect_event(|e| assert_matches!(e, EventType::SubmittedTransaction { .. }))
            .expect_event(|e| assert_matches!(e, EventType::SubmittedTransaction { .. }))
            .expect_event(|e| assert_matches!(e, EventType::SubmittedTransaction { .. }))
            .expect_event(|e| assert_matches!(e, EventType::SucceededTransaction { .. }))
            .expect_event(|e| assert_matches!(e, EventType::SucceededTransaction { .. }))
            .assert_no_more_events();

        read_state(|s| {
            assert_eq!(s.submitted_transactions().len(), 1);
            assert!(s.submitted_transactions().contains_key(&sig_b));
        });
    }

    fn confirmed_status(slot: Slot) -> TransactionStatus {
        TransactionStatus {
            slot,
            status: Ok(()),
            err: None,
            confirmation_status: Some(TransactionConfirmationStatus::Confirmed),
        }
    }

    fn processed_status(slot: Slot) -> TransactionStatus {
        TransactionStatus {
            slot,
            status: Ok(()),
            err: None,
            confirmation_status: Some(TransactionConfirmationStatus::Processed),
        }
    }

    fn finalized_status(slot: Slot) -> TransactionStatus {
        TransactionStatus {
            slot,
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

        let old_signature = signature(0x01);
        add_submitted_transaction(old_signature, EXPIRED_SLOT);

        let new_signature = signature(0xAA);

        let runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            // getSignatureStatuses: not found
            .add_stub_response(SignatureStatusesResult::Consistent(Ok(vec![None])))
            // get_recent_slot_and_blockhash for expiry check (getSlot + getBlock)
            .add_stub_response(SlotResult::Consistent(Ok(CURRENT_SLOT)))
            .add_stub_response(BlockResult::Consistent(Ok(confirmed_block())))
            // get_recent_slot_and_blockhash for resubmission (getSlot + getBlock)
            .add_stub_response(SlotResult::Consistent(Ok(RESUBMISSION_SLOT)))
            .add_stub_response(BlockResult::Consistent(Ok(confirmed_block())))
            .add_stub_response(SendTransactionResult::Consistent(Ok(new_signature.into())))
            .add_signature(new_signature.into());

        monitor_submitted_transactions(runtime).await;

        EventsAssert::from_recorded()
            .expect_event(|e| assert_matches!(e, EventType::SubmittedTransaction { .. }))
            .expect_event(|e| {
                assert_matches!(
                    e,
                    EventType::ResubmittedTransaction {
                        old_signature: old_sig,
                        new_signature: new_sig,
                        new_slot: slot,
                    } if old_sig == old_signature && new_sig == new_signature && slot == RESUBMISSION_SLOT
                )
            })
            .assert_no_more_events();

        read_state(|s| {
            assert_eq!(s.submitted_transactions().len(), 1);
            assert!(s.submitted_transactions().contains_key(&new_signature));
        });
    }

    #[tokio::test]
    async fn should_not_resubmit_expired_transaction_if_status_check_fails() {
        setup();

        add_submitted_transaction(signature(0x01), EXPIRED_SLOT);

        let runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .add_stub_response(SignatureStatusesResult::Consistent(Err(
                RpcError::ValidationError("Error".to_string()),
            )));

        monitor_submitted_transactions(runtime).await;

        EventsAssert::from_recorded()
            .expect_event(|e| assert_matches!(e, EventType::SubmittedTransaction { .. }))
            .assert_no_more_events();

        read_state(|s| assert_eq!(s.submitted_transactions().len(), 1));
    }

    #[tokio::test]
    async fn should_record_resubmission_event_even_if_submission_fails() {
        setup();

        let old_signature = signature(0x01);
        add_submitted_transaction(old_signature, EXPIRED_SLOT);

        let new_signature = signature(0xAA);

        let runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .add_stub_response(SignatureStatusesResult::Consistent(Ok(vec![None])))
            // get_recent_slot_and_blockhash for expiry check (getSlot + getBlock)
            .add_stub_response(SlotResult::Consistent(Ok(CURRENT_SLOT)))
            .add_stub_response(BlockResult::Consistent(Ok(confirmed_block())))
            // get_recent_slot_and_blockhash for resubmission (getSlot + getBlock)
            .add_stub_response(SlotResult::Consistent(Ok(RESUBMISSION_SLOT)))
            .add_stub_response(BlockResult::Consistent(Ok(confirmed_block())))
            .add_stub_response(SendTransactionResult::Inconsistent(vec![]))
            .add_signature(new_signature.into());

        monitor_submitted_transactions(runtime).await;

        EventsAssert::from_recorded()
            .expect_event(|e| assert_matches!(e, EventType::SubmittedTransaction { .. }))
            .expect_event(|e| {
                assert_matches!(
                    e,
                    EventType::ResubmittedTransaction {
                        old_signature: old_sig,
                        new_signature: new_sig,
                        new_slot: slot,
                    } if old_sig == old_signature && new_sig == new_signature && slot == RESUBMISSION_SLOT
                )
            })
            .assert_no_more_events();
    }

    #[tokio::test]
    async fn should_resubmit_multiple_expired_transactions_in_batches() {
        use crate::constants::MAX_CONCURRENT_RPC_CALLS;

        setup();

        let num_transactions = MAX_CONCURRENT_RPC_CALLS + 2; // 10+2 = 12 transactions require 2 rounds
        for i in 0..num_transactions {
            add_submitted_transaction(signature(i as u8), EXPIRED_SLOT);
        }

        let mut runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            // getSignatureStatuses: all not found
            .add_stub_response(SignatureStatusesResult::Consistent(Ok(
                vec![None; num_transactions],
            )))
            // get_recent_slot_and_blockhash for expiry check (getSlot + getBlock)
            .add_stub_response(SlotResult::Consistent(Ok(CURRENT_SLOT)))
            .add_stub_response(BlockResult::Consistent(Ok(confirmed_block())))
            // Round 1: get_recent_slot_and_blockhash for resubmission (getSlot + getBlock)
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

        // Round 2: get_recent_slot_and_blockhash for resubmission (getSlot + getBlock)
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

fn add_submitted_transaction(signature: solana_signature::Signature, slot: Slot) {
    events::submit_consolidation(signature, MINTER_ACCOUNT, slot, vec![]);
}
