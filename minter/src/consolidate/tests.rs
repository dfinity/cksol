use super::{MAX_TRANSFERS_PER_CONSOLIDATION, consolidate_deposits};
use crate::{
    numeric::LedgerMintIndex,
    state::{
        TaskType,
        audit::process_event,
        event::{DepositId, EventType, TransactionPurpose},
        mutate_state,
    },
    test_fixtures::{
        DEPOSIT_FEE, EventsAssert, init_schnorr_master_key, init_state,
        runtime::TestCanisterRuntime,
    },
};
use assert_matches::assert_matches;
use candid::Principal;
use icrc_ledger_types::icrc1::account::Account;
use sol_rpc_types::{ConfirmedBlock, MultiRpcResult, RpcError, Slot};
use solana_signature::Signature;

type SlotResult = MultiRpcResult<Slot>;
type BlockResult = MultiRpcResult<ConfirmedBlock>;
type SendTransactionResult = MultiRpcResult<sol_rpc_types::Signature>;

#[tokio::test]
async fn should_return_early_if_no_deposits_to_consolidate() {
    setup();

    consolidate_deposits(TestCanisterRuntime::new()).await;

    EventsAssert::assert_no_events_recorded();
}

#[tokio::test]
async fn should_return_early_if_task_already_active() {
    setup();

    add_funds_to_consolidate(vec![(account(0), 1_000_000_000)]);
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

    add_funds_to_consolidate(vec![(account(0), 1_000_000_000)]);

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

    add_funds_to_consolidate(vec![(account(0), 1_000_000_000)]);

    let fee_payer_signature = Signature::from([0x11; 64]);
    let slot = 100;
    let runtime = TestCanisterRuntime::new()
        .with_increasing_time()
        // get_recent_blockhash calls
        .add_stub_response(SlotResult::Consistent(Ok(slot)))
        .add_stub_response(BlockResult::Consistent(Ok(block())))
        // get_slot call
        .add_stub_response(SlotResult::Consistent(Ok(slot)))
        .add_stub_response(SendTransactionResult::Consistent(Ok(
            fee_payer_signature.into()
        )))
        .add_signature(fee_payer_signature.into())
        .add_signature([0x22; 64]);

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

    add_funds_to_consolidate(vec![(account(0), 1_000_000_000)]);

    let fee_payer_signature = Signature::from([0x11; 64]);
    let slot = 100;
    let runtime = TestCanisterRuntime::new()
        .with_increasing_time()
        // get_recent_blockhash calls
        .add_stub_response(SlotResult::Consistent(Ok(slot)))
        .add_stub_response(BlockResult::Consistent(Ok(block())))
        // get_slot call
        .add_stub_response(SlotResult::Consistent(Ok(slot)))
        // Transaction submission fails
        .add_stub_response(SendTransactionResult::Inconsistent(vec![]))
        .add_signature(fee_payer_signature.into())
        .add_signature([0x22; 64]);

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
        .map(|i| (account(i as u8), (i as u64 + 1) * 1_000_000_000))
        .collect();
    add_funds_to_consolidate(funds.clone());

    const BATCH_1_SIZE: usize = MAX_TRANSFERS_PER_CONSOLIDATION;

    let fee_payer_signature_1 = Signature::from([0; 64]);
    let fee_payer_signature_2 = Signature::from([(BATCH_1_SIZE + 1) as u8; 64]);
    let slot = 100;

    let mut runtime = TestCanisterRuntime::new()
        .with_increasing_time()
        // get_recent_blockhash calls
        .add_stub_response(SlotResult::Consistent(Ok(slot)))
        .add_stub_response(BlockResult::Consistent(Ok(block())))
        // get_slot call
        .add_stub_response(SlotResult::Consistent(Ok(slot)))
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

    let deposit_account = account(0);
    // Two deposits to the same account
    add_funds_to_consolidate(vec![
        (deposit_account, 500_000_000),
        (deposit_account, 300_000_000),
    ]);

    let fee_payer_signature = Signature::from([0x11; 64]);
    let slot = 100;
    let runtime = TestCanisterRuntime::new()
        .with_increasing_time()
        .add_stub_response(SlotResult::Consistent(Ok(slot)))
        .add_stub_response(BlockResult::Consistent(Ok(block())))
        .add_stub_response(SlotResult::Consistent(Ok(slot)))
        .add_stub_response(SendTransactionResult::Consistent(Ok(
            fee_payer_signature.into()
        )))
        // Only TWO signatures: fee payer + one source account (not two)
        .add_signature(fee_payer_signature.into())
        .add_signature([0x22; 64]);

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

fn setup() {
    init_state();
    init_schnorr_master_key();
}

fn account(i: u8) -> Account {
    Account {
        owner: Principal::from_slice(&[i; 29]),
        subaccount: None,
    }
}

fn add_funds_to_consolidate(funds: Vec<(Account, u64)>) {
    for (i, (account, amount)) in funds.into_iter().enumerate() {
        let deposit_id = DepositId {
            account,
            signature: Signature::from([i as u8; 64]),
        };
        let mint_block_index = LedgerMintIndex::from(i as u64);
        let runtime = TestCanisterRuntime::new().with_increasing_time();
        mutate_state(|state| {
            process_event(
                state,
                EventType::AcceptedDeposit {
                    deposit_id,
                    deposit_amount: amount,
                    amount_to_mint: amount - DEPOSIT_FEE,
                },
                &runtime,
            )
        });
        mutate_state(|state| {
            process_event(
                state,
                EventType::Minted {
                    deposit_id,
                    mint_block_index,
                },
                &runtime,
            )
        });
    }
}

fn block() -> ConfirmedBlock {
    ConfirmedBlock {
        previous_blockhash: Default::default(),
        blockhash: solana_hash::Hash::from([0x42; 32]).into(),
        parent_slot: 0,
        block_time: None,
        block_height: None,
        signatures: None,
        rewards: None,
        num_reward_partitions: None,
        transactions: None,
    }
}
