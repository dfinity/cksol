use super::*;
use crate::{
    rpc_executor::{execute_rpc_queue, reset_work_queue},
    state::{event::EventType, read_state, reset_state},
    storage::reset_events,
    test_fixtures::{
        EventsAssert, MINTER_ACCOUNT, confirmed_block, deposit_id, events, init_schnorr_master_key,
        init_state, runtime::TestCanisterRuntime, signature,
    },
};
use sol_rpc_types::{
    ConfirmedBlock, MultiRpcResult, RpcError, Slot, TransactionConfirmationStatus,
    TransactionError, TransactionStatus,
};

type SlotResult = MultiRpcResult<Slot>;
type BlockResult = MultiRpcResult<ConfirmedBlock>;
type SignatureStatusesResult = MultiRpcResult<Vec<Option<TransactionStatus>>>;

const CURRENT_SLOT: Slot = 408_807_102;
const RECENT_SLOT: Slot = CURRENT_SLOT - 10;
const EXPIRED_SLOT: Slot = CURRENT_SLOT - MAX_BLOCKHASH_AGE - 1;

fn setup() {
    reset_state();
    reset_events();
    reset_work_queue();
    init_state();
    init_schnorr_master_key();
}

fn finalized_status() -> TransactionStatus {
    TransactionStatus {
        slot: CURRENT_SLOT,
        status: Ok(()),
        err: None,
        confirmation_status: Some(TransactionConfirmationStatus::Finalized),
    }
}

fn errored_status() -> TransactionStatus {
    TransactionStatus {
        slot: CURRENT_SLOT,
        status: Err(TransactionError::AccountNotFound),
        err: Some(TransactionError::AccountNotFound),
        confirmation_status: Some(TransactionConfirmationStatus::Finalized),
    }
}

/// Submit a consolidation transaction to state so the executor can transition it.
fn submit_to_state(i: usize, sig: solana_signature::Signature, slot: Slot) {
    events::accept_deposit(deposit_id(i), 1_000_000);
    events::mint_deposit(deposit_id(i), i as u64);
    events::submit_consolidation(sig, MINTER_ACCOUNT, slot, vec![i as u64]);
}

// ---------------------------------------------------------------------------
// CheckSignatureStatuses
// ---------------------------------------------------------------------------

#[tokio::test]
async fn should_mark_finalized_transaction_as_succeeded() {
    setup();

    let sig = signature(0);
    submit_to_state(0, sig, RECENT_SLOT);
    enqueue(WorkItem::CheckSignatureStatuses {
        signatures: vec![sig],
        submitted_slots: [(sig, RECENT_SLOT)].into_iter().collect(),
    });

    let runtime = TestCanisterRuntime::new()
        .with_increasing_time()
        .add_stub_response(SlotResult::Consistent(Ok(CURRENT_SLOT)))
        .add_stub_response(BlockResult::Consistent(Ok(confirmed_block())))
        .add_stub_response(SignatureStatusesResult::Consistent(Ok(vec![Some(
            finalized_status(),
        )])));

    execute_rpc_queue(runtime).await;

    EventsAssert::from_recorded()
        .expect_contains_event_eq(EventType::SucceededTransaction { signature: sig });

    read_state(|s| {
        assert!(s.submitted_transactions().is_empty());
        assert!(s.succeeded_transactions().contains(&sig));
    });
}

#[tokio::test]
async fn should_mark_finalized_transaction_with_error_as_failed() {
    setup();

    let sig = signature(0);
    submit_to_state(0, sig, RECENT_SLOT);
    enqueue(WorkItem::CheckSignatureStatuses {
        signatures: vec![sig],
        submitted_slots: [(sig, RECENT_SLOT)].into_iter().collect(),
    });

    let runtime = TestCanisterRuntime::new()
        .with_increasing_time()
        .add_stub_response(SlotResult::Consistent(Ok(CURRENT_SLOT)))
        .add_stub_response(BlockResult::Consistent(Ok(confirmed_block())))
        .add_stub_response(SignatureStatusesResult::Consistent(Ok(vec![Some(
            errored_status(),
        )])));

    execute_rpc_queue(runtime).await;

    EventsAssert::from_recorded()
        .expect_contains_event_eq(EventType::FailedTransaction { signature: sig });

    read_state(|s| {
        assert!(s.submitted_transactions().is_empty());
        assert_eq!(s.failed_transactions().len(), 1);
    });
}

#[tokio::test]
async fn should_mark_missing_transaction_as_expired_when_old_enough() {
    setup();

    let sig = signature(0);
    submit_to_state(0, sig, EXPIRED_SLOT);
    enqueue(WorkItem::CheckSignatureStatuses {
        signatures: vec![sig],
        submitted_slots: [(sig, EXPIRED_SLOT)].into_iter().collect(),
    });

    let runtime = TestCanisterRuntime::new()
        .with_increasing_time()
        .add_stub_response(SlotResult::Consistent(Ok(CURRENT_SLOT)))
        .add_stub_response(BlockResult::Consistent(Ok(confirmed_block())))
        .add_stub_response(SignatureStatusesResult::Consistent(Ok(vec![None])));

    execute_rpc_queue(runtime).await;

    EventsAssert::from_recorded()
        .expect_contains_event_eq(EventType::ExpiredTransaction { signature: sig });

    read_state(|s| {
        assert!(s.submitted_transactions().is_empty());
        assert!(s.transactions_to_resubmit().contains_key(&sig));
    });
}

#[tokio::test]
async fn should_not_expire_recent_missing_transaction() {
    setup();

    let sig = signature(0);
    submit_to_state(0, sig, RECENT_SLOT);
    enqueue(WorkItem::CheckSignatureStatuses {
        signatures: vec![sig],
        submitted_slots: [(sig, RECENT_SLOT)].into_iter().collect(),
    });

    let events_before = EventsAssert::from_recorded();

    let runtime = TestCanisterRuntime::new()
        .with_increasing_time()
        .add_stub_response(SlotResult::Consistent(Ok(CURRENT_SLOT)))
        .add_stub_response(BlockResult::Consistent(Ok(confirmed_block())))
        .add_stub_response(SignatureStatusesResult::Consistent(Ok(vec![None])));

    execute_rpc_queue(runtime).await;

    // No new events: transaction remains submitted (not expired or succeeded)
    let events_after = EventsAssert::from_recorded();
    assert_eq!(events_before, events_after);

    read_state(|s| {
        assert!(s.submitted_transactions().contains_key(&sig));
    });
}

#[tokio::test]
async fn should_re_enqueue_on_blockhash_fetch_failure() {
    setup();

    let sig = signature(0);
    submit_to_state(0, sig, RECENT_SLOT);
    enqueue(WorkItem::CheckSignatureStatuses {
        signatures: vec![sig],
        submitted_slots: [(sig, RECENT_SLOT)].into_iter().collect(),
    });

    let events_before = EventsAssert::from_recorded();

    let error = SlotResult::Consistent(Err(RpcError::ValidationError("Error".to_string())));
    let runtime = TestCanisterRuntime::new()
        .add_stub_response(error.clone())
        .add_stub_response(error.clone())
        .add_stub_response(error);

    execute_rpc_queue(runtime).await;

    // No new state events; item was re-enqueued for retry
    let events_after = EventsAssert::from_recorded();
    assert_eq!(events_before, events_after);
    assert!(!queue_is_empty(), "item should have been re-enqueued");
}

#[tokio::test]
async fn should_process_multiple_items_in_one_batch() {
    setup();

    let sig0 = signature(0);
    let sig1 = signature(1);
    submit_to_state(0, sig0, RECENT_SLOT);
    submit_to_state(1, sig1, RECENT_SLOT);

    enqueue(WorkItem::CheckSignatureStatuses {
        signatures: vec![sig0],
        submitted_slots: [(sig0, RECENT_SLOT)].into_iter().collect(),
    });
    enqueue(WorkItem::CheckSignatureStatuses {
        signatures: vec![sig1],
        submitted_slots: [(sig1, RECENT_SLOT)].into_iter().collect(),
    });

    let runtime = TestCanisterRuntime::new()
        .with_increasing_time()
        .add_stub_response(SlotResult::Consistent(Ok(CURRENT_SLOT)))
        .add_stub_response(BlockResult::Consistent(Ok(confirmed_block())))
        .add_stub_response(SignatureStatusesResult::Consistent(Ok(vec![Some(
            finalized_status(),
        )])))
        .add_stub_response(SignatureStatusesResult::Consistent(Ok(vec![Some(
            finalized_status(),
        )])));

    execute_rpc_queue(runtime).await;

    EventsAssert::from_recorded()
        .expect_contains_event_eq(EventType::SucceededTransaction { signature: sig0 })
        .expect_contains_event_eq(EventType::SucceededTransaction { signature: sig1 });
}
