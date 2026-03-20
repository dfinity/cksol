use super::{MAX_TRANSFERS_PER_CONSOLIDATION, consolidate_deposits};
use crate::{
    state::{
        TaskType,
        audit::process_event,
        event::{DepositId, EventType},
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
async fn should_return_early_if_no_funds_to_consolidate() {
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

    // Only AcceptedDeposit event from setup, no consolidation events
    EventsAssert::from_recorded()
        .expect_event(|e| assert_matches!(e, EventType::AcceptedDeposit { .. }))
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

    // Only AcceptedDeposit event from setup, no consolidation events
    EventsAssert::from_recorded()
        .expect_event(|e| assert_matches!(e, EventType::AcceptedDeposit { .. }))
        .assert_no_more_events();
}

#[tokio::test]
async fn should_submit_single_consolidation_request() {
    setup();

    let deposit_account = account(0);
    let deposit_amount = 1_000_000_000_u64;
    add_funds_to_consolidate(vec![(deposit_account, deposit_amount)]);

    // Fee payer signature is first in the transaction and becomes the transaction ID
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
        // Two signatures needed: fee payer (minter) + source (deposit account)
        .add_signature(fee_payer_signature.into())
        .add_signature([0x22; 64]);

    consolidate_deposits(runtime).await;

    EventsAssert::from_recorded()
        .expect_event(|e| {
            assert_matches!(
                e,
                EventType::AcceptedDeposit {
                    deposit_id,
                    deposit_amount: amount,
                    ..
                } if deposit_id.account == deposit_account && amount == deposit_amount
            )
        })
        .expect_event(|e| {
            assert_matches!(e, EventType::ConsolidatedDeposits { deposits }
                if deposits == vec![(deposit_account, deposit_amount)]
            )
        })
        .expect_event(|e| {
            assert_matches!(e, EventType::SubmittedTransaction { signature, slot: event_slot, .. }
                if signature == fee_payer_signature && event_slot == slot
            )
        })
        .assert_no_more_events();
}

#[tokio::test]
async fn should_record_events_even_if_transaction_submission_fails() {
    setup();

    let deposit_account = account(0);
    let deposit_amount = 1_000_000_000_u64;
    add_funds_to_consolidate(vec![(deposit_account, deposit_amount)]);

    let fee_payer_signature = Signature::from([0x11; 64]);
    let slot = 100;
    let runtime = TestCanisterRuntime::new()
        .with_increasing_time()
        // get_recent_blockhash calls
        .add_stub_response(SlotResult::Consistent(Ok(slot)))
        .add_stub_response(BlockResult::Consistent(Ok(block())))
        // get_slot call
        .add_stub_response(SlotResult::Consistent(Ok(slot)))
        // Transaction submission call fails (e.g. due to inconsistent results)
        .add_stub_response(SendTransactionResult::Inconsistent(vec![]))
        .add_signature(fee_payer_signature.into())
        .add_signature([0x22; 64]);

    consolidate_deposits(runtime).await;

    EventsAssert::from_recorded()
        .expect_event(|e| {
            assert_matches!(e, EventType::AcceptedDeposit { deposit_id, deposit_amount: amount, ..}
                if deposit_id.account == deposit_account && amount == deposit_amount
            )
        })
        .expect_event(|e| {
            assert_matches!(e, EventType::ConsolidatedDeposits { deposits }
                if deposits == vec![(deposit_account, deposit_amount)]
            )
        })
        .expect_event(|e| {
            assert_matches!(e, EventType::SubmittedTransaction { signature, slot: event_slot, .. }
                if signature == fee_payer_signature && event_slot == slot
            )
        })
        .assert_no_more_events();
}

#[tokio::test]
async fn should_submit_multiple_consolidation_batches() {
    const NUM_DEPOSITS: usize = 11;
    setup();

    let funds: Vec<_> = (0..NUM_DEPOSITS)
        .map(|i| (account(i as u8), (i as u64 + 1) * 1_000_000_000))
        .collect();
    add_funds_to_consolidate(funds.clone());

    // Calculate expected batch sizes, i.e. the number of transfers per transaction submitted
    let batch_1_size = MAX_TRANSFERS_PER_CONSOLIDATION; // 9 accounts
    let batch_2_size = NUM_DEPOSITS - batch_1_size; // 2 accounts

    // Fee payer signatures (first signature in each batch) become transaction IDs
    let fee_payer_signature_1 = Signature::from([0x00; 64]); // index 0
    let fee_payer_signature_2 = Signature::from([0x0A; 64]); // index 10
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

    // Signatures needed: fee payer + each source account per batch
    for i in 0..13 {
        runtime = runtime.add_signature([i as u8; 64]);
    }

    consolidate_deposits(runtime).await;

    let mut events_assert = EventsAssert::from_recorded();
    // AcceptedDeposit events from setup
    for (account, amount) in funds.iter().cloned() {
        events_assert = events_assert.expect_event(move |e| {
            assert_matches!(e, EventType::AcceptedDeposit { deposit_id, deposit_amount, .. }
                if deposit_id.account == account && deposit_amount == amount
            )
        });
    }
    // Batch 1: 9 deposits consolidated together
    events_assert = events_assert
        .expect_event(|e| {
            assert_matches!(e, EventType::ConsolidatedDeposits { deposits }
                    if deposits.len() == batch_1_size
            )
        })
        .expect_event(|e| {
            assert_matches!(e, EventType::SubmittedTransaction { signature, slot: event_slot, .. }
                if signature == fee_payer_signature_1 && event_slot == slot
            )
        });
    // Batch 2: 2 deposits consolidated together
    events_assert = events_assert
        .expect_event(|e| {
            assert_matches!(e, EventType::ConsolidatedDeposits { deposits }
                if deposits.len() == batch_2_size
            )
        })
        .expect_event(|e| {
            assert_matches!(e, EventType::SubmittedTransaction { signature, slot: event_slot, .. }
                if signature == fee_payer_signature_2 && event_slot == slot
            )
        });
    events_assert.assert_no_more_events();
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
        mutate_state(|state| {
            process_event(
                state,
                EventType::AcceptedDeposit {
                    deposit_id,
                    deposit_amount: amount,
                    amount_to_mint: amount - DEPOSIT_FEE,
                },
                &TestCanisterRuntime::new().with_increasing_time(),
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
