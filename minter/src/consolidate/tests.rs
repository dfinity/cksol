use super::{MAX_TRANSFERS_PER_CONSOLIDATION, consolidate_deposits};
use crate::{
    constants::MAX_CONCURRENT_RPC_CALLS,
    numeric::LedgerMintIndex,
    state::{
        TaskType,
        event::{DepositId, EventType, TransactionPurpose},
        mutate_state, read_state,
    },
    test_fixtures::{
        EventsAssert, account, confirmed_block, deposit_id,
        events::{accept_deposit, mint_deposit},
        init_schnorr_master_key, init_state,
        runtime::TestCanisterRuntime,
        signature,
    },
};
use assert_matches::assert_matches;
use sol_rpc_types::{ConfirmedBlock, MultiRpcResult, RpcError, Signature, Slot};

type SlotResult = MultiRpcResult<Slot>;
type BlockResult = MultiRpcResult<ConfirmedBlock>;
type SendTransactionResult = MultiRpcResult<Signature>;

#[tokio::test]
async fn should_return_early_if_no_deposits_to_consolidate() {
    setup();

    consolidate_deposits(TestCanisterRuntime::new()).await;

    EventsAssert::assert_no_events_recorded();
}

#[tokio::test]
async fn should_return_early_if_task_already_active() {
    setup();

    add_funds_to_consolidate(&[(deposit_id(0), 1_000_000_000)]);
    mutate_state(|s| {
        s.active_tasks_mut().insert(TaskType::DepositConsolidation);
    });

    consolidate_deposits(TestCanisterRuntime::new()).await;

    // Only events from setup
    EventsAssert::from_recorded()
        .expect_event(|e| assert_matches!(e, EventType::AcceptedDeposit { .. }))
        .expect_event(|e| assert_matches!(e, EventType::Minted { .. }))
        .assert_no_more_events();
}

#[tokio::test]
async fn should_return_early_if_fetching_blockhash_fails() {
    setup();

    add_funds_to_consolidate(&[(deposit_id(0), 1_000_000_000)]);

    let error = SlotResult::Consistent(Err(RpcError::ValidationError("Error".to_string())));
    let runtime = TestCanisterRuntime::new()
        .add_stub_response(error.clone())
        .add_stub_response(error.clone())
        .add_stub_response(error);

    consolidate_deposits(runtime).await;

    // Only events from setup
    EventsAssert::from_recorded()
        .expect_event(|e| assert_matches!(e, EventType::AcceptedDeposit { .. }))
        .expect_event(|e| assert_matches!(e, EventType::Minted { .. }))
        .assert_no_more_events();
}

#[tokio::test]
async fn should_submit_single_consolidation_request() {
    setup();

    add_funds_to_consolidate(&[(deposit_id(0), 1_000_000_000)]);

    let fee_payer_signature = signature(0x11);
    let slot = 100;
    let runtime = TestCanisterRuntime::new()
        .with_increasing_time()
        // get_recent_slot_and_blockhash calls (get_recent_block internally calls getSlot then getBlock)
        .add_stub_response(SlotResult::Consistent(Ok(slot)))
        .add_stub_response(BlockResult::Consistent(Ok(confirmed_block())))
        .add_stub_response(SendTransactionResult::Consistent(Ok(
            fee_payer_signature.into()
        )))
        .add_signature(fee_payer_signature.into());

    consolidate_deposits(runtime).await;

    EventsAssert::from_recorded()
        .expect_event(|e| assert_matches!(e, EventType::AcceptedDeposit { .. }))
        .expect_event(|e| assert_matches!(e, EventType::Minted { .. }))
        .expect_event(|e| {
            assert_matches!(e, EventType::SubmittedTransaction {
                signature,
                slot: event_slot,
                purpose: TransactionPurpose::ConsolidateDeposits { mint_indices },
                ..
            } if signature == fee_payer_signature
              && event_slot == slot
              && mint_indices == vec![LedgerMintIndex::from(0_u64)]
            )
        })
        .assert_no_more_events();
}

#[tokio::test]
async fn should_record_events_even_if_transaction_submission_fails() {
    setup();

    add_funds_to_consolidate(&[(deposit_id(0), 1_000_000_000)]);

    let fee_payer_signature = signature(0x11);
    let slot = 100;
    let runtime = TestCanisterRuntime::new()
        .with_increasing_time()
        // get_recent_slot_and_blockhash calls
        .add_stub_response(SlotResult::Consistent(Ok(slot)))
        .add_stub_response(BlockResult::Consistent(Ok(confirmed_block())))
        // Transaction submission fails
        .add_stub_response(SendTransactionResult::Inconsistent(vec![]))
        .add_signature(fee_payer_signature.into());

    consolidate_deposits(runtime).await;

    EventsAssert::from_recorded()
        .expect_event(|e| assert_matches!(e, EventType::AcceptedDeposit { .. }))
        .expect_event(|e| assert_matches!(e, EventType::Minted { .. }))
        .expect_event(|e| {
            assert_matches!(e, EventType::SubmittedTransaction {
                purpose: TransactionPurpose::ConsolidateDeposits { mint_indices },
                ..
            } if mint_indices == vec![LedgerMintIndex::from(0_u64)]
            )
        })
        .assert_no_more_events();
}

#[tokio::test]
async fn should_submit_multiple_consolidation_batches() {
    const NUM_DEPOSITS: usize = MAX_TRANSFERS_PER_CONSOLIDATION + 1;
    setup();

    let funds: Vec<_> = (0..NUM_DEPOSITS)
        .map(|i| (deposit_id(i), (i as u64 + 1) * 1_000_000_000))
        .collect();
    add_funds_to_consolidate(&funds);

    const BATCH_1_SIZE: usize = MAX_TRANSFERS_PER_CONSOLIDATION;

    let fee_payer_signature_1 = signature(0);
    let fee_payer_signature_2 = signature(BATCH_1_SIZE);
    let slot = 100;

    let mut runtime = TestCanisterRuntime::new()
        .with_increasing_time()
        // get_recent_slot_and_blockhash calls
        .add_stub_response(SlotResult::Consistent(Ok(slot)))
        .add_stub_response(BlockResult::Consistent(Ok(confirmed_block())))
        .add_stub_response(SendTransactionResult::Consistent(Ok(
            fee_payer_signature_1.into()
        )))
        .add_stub_response(SendTransactionResult::Consistent(Ok(
            fee_payer_signature_2.into()
        )));

    for i in 0..(2 + NUM_DEPOSITS) {
        runtime = runtime.add_signature([i as u8; 64]);
    }

    consolidate_deposits(runtime).await;

    let mut events_assert = EventsAssert::from_recorded();
    // Events from setup
    for _ in 0..NUM_DEPOSITS {
        events_assert = events_assert
            .expect_event(|e| assert_matches!(e, EventType::AcceptedDeposit { .. }))
            .expect_event(|e| assert_matches!(e, EventType::Minted { .. }));
    }
    // Batch 1:
    let batch_1_indices: Vec<_> = (0..BATCH_1_SIZE as u64)
        .map(LedgerMintIndex::from)
        .collect();
    events_assert = events_assert.expect_event(move |e| {
        assert_matches!(e, EventType::SubmittedTransaction {
            signature,
            purpose: TransactionPurpose::ConsolidateDeposits { mint_indices },
            ..
        } if signature == fee_payer_signature_1
          && mint_indices == batch_1_indices
        )
    });
    // Batch 2:
    let batch_2_indices: Vec<_> = (BATCH_1_SIZE as u64..NUM_DEPOSITS as u64)
        .map(LedgerMintIndex::from)
        .collect();
    events_assert = events_assert.expect_event(move |e| {
        assert_matches!(e, EventType::SubmittedTransaction {
            signature,
            purpose: TransactionPurpose::ConsolidateDeposits { mint_indices },
            ..
        } if signature == fee_payer_signature_2
          && mint_indices == batch_2_indices
        )
    });
    events_assert.assert_no_more_events();
}

#[tokio::test]
async fn should_consolidate_multiple_deposits_to_same_account_in_single_transfer() {
    setup();

    // Two deposits to the same account (different signatures)
    let same_account = account(0);
    let deposit_a = DepositId {
        account: same_account,
        signature: signature(0),
    };
    let deposit_b = DepositId {
        account: same_account,
        signature: signature(1),
    };
    add_funds_to_consolidate(&[(deposit_a, 500_000_000), (deposit_b, 300_000_000)]);

    let fee_payer_signature = signature(0x11);
    let slot = 100;
    let runtime = TestCanisterRuntime::new()
        .with_increasing_time()
        .add_stub_response(SlotResult::Consistent(Ok(slot)))
        .add_stub_response(BlockResult::Consistent(Ok(confirmed_block())))
        .add_stub_response(SendTransactionResult::Consistent(Ok(
            fee_payer_signature.into()
        )))
        .add_signature(fee_payer_signature.into());

    consolidate_deposits(runtime).await;

    EventsAssert::from_recorded()
        .expect_event(|e| assert_matches!(e, EventType::AcceptedDeposit { .. }))
        .expect_event(|e| assert_matches!(e, EventType::Minted { .. }))
        .expect_event(|e| assert_matches!(e, EventType::AcceptedDeposit { .. }))
        .expect_event(|e| assert_matches!(e, EventType::Minted { .. }))
        .expect_event(|e| {
            assert_matches!(e, EventType::SubmittedTransaction {
                signature,
                purpose: TransactionPurpose::ConsolidateDeposits { mint_indices },
                ..
            } if signature == fee_payer_signature
              && mint_indices == vec![LedgerMintIndex::from(0_u64), LedgerMintIndex::from(1_u64)]
            )
        })
        .assert_no_more_events();
}

#[tokio::test]
async fn should_consolidate_up_to_max_concurrent_batches_per_invocation() {
    setup();

    // Create enough deposits to require more than MAX_CONCURRENT_RPC_CALLS batches
    let num_deposits = MAX_TRANSFERS_PER_CONSOLIDATION * MAX_CONCURRENT_RPC_CALLS + 1;
    let funds: Vec<_> = (0..num_deposits)
        .map(|i| (deposit_id(i), (i as u64 + 1) * 1_000_000_000))
        .collect();
    add_funds_to_consolidate(&funds);

    let slot = 100;
    let mut runtime = TestCanisterRuntime::new()
        .with_increasing_time()
        .add_stub_response(SlotResult::Consistent(Ok(slot)))
        .add_stub_response(BlockResult::Consistent(Ok(confirmed_block())));

    // Provide signatures and send responses for MAX_CONCURRENT_RPC_CALLS batches
    for i in 0..MAX_CONCURRENT_RPC_CALLS {
        runtime =
            runtime.add_stub_response(SendTransactionResult::Consistent(Ok(signature(i).into())));
    }
    for i in 0..(MAX_CONCURRENT_RPC_CALLS + num_deposits) {
        runtime = runtime.add_signature([i as u8; 64]);
    }

    consolidate_deposits(runtime).await;

    // Only MAX_CONCURRENT_RPC_CALLS batches should have been submitted
    read_state(|s| {
        assert_eq!(s.submitted_transactions().len(), MAX_CONCURRENT_RPC_CALLS);
        assert!(
            !s.deposits_to_consolidate().is_empty(),
            "Some deposits should remain unconsolidated"
        );
    });
}

#[tokio::test]
async fn should_reschedule_immediately_when_deposits_remain() {
    setup();

    let num_deposits = MAX_TRANSFERS_PER_CONSOLIDATION * MAX_CONCURRENT_RPC_CALLS + 1;
    let funds: Vec<_> = (0..num_deposits)
        .map(|i| (deposit_id(i), (i as u64 + 1) * 1_000_000_000))
        .collect();
    add_funds_to_consolidate(&funds);

    let slot = 100;
    let mut runtime = TestCanisterRuntime::new()
        .with_increasing_time()
        .add_stub_response(SlotResult::Consistent(Ok(slot)))
        .add_stub_response(BlockResult::Consistent(Ok(confirmed_block())));
    for i in 0..MAX_CONCURRENT_RPC_CALLS {
        runtime =
            runtime.add_stub_response(SendTransactionResult::Consistent(Ok(signature(i).into())));
    }
    for i in 0..(MAX_CONCURRENT_RPC_CALLS + num_deposits) {
        runtime = runtime.add_signature([i as u8; 64]);
    }

    consolidate_deposits(runtime.clone()).await;

    assert_eq!(
        runtime.set_timer_call_count(),
        1,
        "should reschedule when deposits remain"
    );
}

#[tokio::test]
async fn should_not_reschedule_when_all_deposits_consolidated() {
    setup();

    add_funds_to_consolidate(&[(deposit_id(0), 1_000_000_000)]);

    let slot = 100;
    let runtime = TestCanisterRuntime::new()
        .with_increasing_time()
        .add_stub_response(SlotResult::Consistent(Ok(slot)))
        .add_stub_response(BlockResult::Consistent(Ok(confirmed_block())))
        .add_stub_response(SendTransactionResult::Consistent(
            Ok(signature(0x11).into()),
        ))
        .add_signature(signature(0x11).into());

    consolidate_deposits(runtime.clone()).await;

    assert_eq!(
        runtime.set_timer_call_count(),
        0,
        "should not reschedule when all deposits consolidated"
    );
}

fn setup() {
    init_state();
    init_schnorr_master_key();
}

fn add_funds_to_consolidate(deposits: &[(DepositId, u64)]) {
    for (i, &(deposit_id, amount)) in deposits.iter().enumerate() {
        accept_deposit(deposit_id, amount);
        mint_deposit(deposit_id, i as u64);
    }
}
