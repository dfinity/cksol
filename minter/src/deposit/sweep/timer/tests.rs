use super::{MAX_DEPOSITS_PER_SWEEP, sweep_queued_deposits};
use crate::{
    address::account_address,
    constants::{FEE_PER_SIGNATURE, MAX_CONCURRENT_RPC_CALLS},
    state::{
        TaskType,
        event::{EventType, Signer, TransactionPurpose, VersionedMessage},
        mutate_state, read_state,
    },
    test_fixtures::{
        DEFAULT_BLOCK_HEIGHT, EventsAssert, MINIMUM_DEPOSIT_AMOUNT, MINTER_ADDRESS, account,
        account_signature, events::queue_deposit, init_schnorr_master_key, init_state,
        runtime::TestCanisterRuntime, schnorr_master_key_response, signer::sign_for,
    },
};
use assert_matches::assert_matches;
use cksol_types::{DepositSolId, DepositSolStatus};
use icrc_ledger_types::icrc1::account::Account;
use sol_rpc_types::{Lamport, MultiRpcResult, RpcError, Signature, Slot};
use solana_address::Address;
use solana_system_interface::instruction::SystemInstruction;

type SendTransactionResult = MultiRpcResult<Signature>;

const SLOT: Slot = 300_000_000;

#[tokio::test]
async fn should_return_early_if_no_deposits_queued() {
    setup();
    let runtime = TestCanisterRuntime::new();

    sweep_queued_deposits(runtime.clone()).await;

    EventsAssert::assert_no_events_recorded();
    assert_eq!(runtime.set_timer_call_count(), 0);
}

#[tokio::test]
async fn should_return_early_if_task_already_active() {
    setup();
    queue_deposit(0, account(1), MINIMUM_DEPOSIT_AMOUNT);
    let events_before = EventsAssert::from_recorded();
    mutate_state(|s| {
        s.active_tasks_mut().insert(TaskType::SweepDeposits);
    });

    sweep_queued_deposits(TestCanisterRuntime::new()).await;

    assert_eq!(events_before, EventsAssert::from_recorded());
}

#[tokio::test]
async fn should_not_submit_if_fetching_blockhash_fails() {
    setup();
    let status_before = queue_deposit(0, account(1), MINIMUM_DEPOSIT_AMOUNT);
    let events_before = EventsAssert::from_recorded();
    let runtime = TestCanisterRuntime::new()
        .add_recent_block(Err(RpcError::ValidationError("Error".to_string())));

    sweep_queued_deposits(runtime.clone()).await;

    assert_eq!(events_before, EventsAssert::from_recorded());
    assert_eq!(status_before, deposit_status(0));
    assert_eq!(runtime.set_timer_call_count(), 0);
}

#[tokio::test]
async fn should_sweep_batch_with_largest_deposit_as_fee_payer() {
    setup();
    let deposits = [
        (0, account(1), 30_000_000),
        (1, account(2), 50_000_000),
        (2, account(3), 40_000_000),
    ];
    for (deposit_id, account, sweepable_amount) in deposits {
        queue_deposit(deposit_id, account, sweepable_amount);
    }
    let fee_payer_signature = account_signature(&account(2));
    let runtime = TestCanisterRuntime::new()
        .with_increasing_time()
        .add_recent_block(Ok(SLOT))
        .transaction_builder(account(2), fee_payer_signature)
        .add_signers([account(1), account(3)])
        .build();

    sweep_queued_deposits(runtime.clone()).await;

    let transaction_fee = FEE_PER_SIGNATURE * deposits.len() as u64;
    EventsAssert::from_recorded()
        .expect_event(|e| assert_matches!(e, EventType::QueuedDeposit { .. }))
        .expect_event(|e| assert_matches!(e, EventType::QueuedDeposit { .. }))
        .expect_event(|e| assert_matches!(e, EventType::QueuedDeposit { .. }))
        .expect_event(|e| {
            assert_matches!(e, EventType::SubmittedTransaction {
                signature,
                message,
                signers,
                purpose: TransactionPurpose::SweepDeposits { deposit_ids },
                block_height,
            } => {
                assert_eq!(signature, fee_payer_signature);
                assert_eq!(signers[0], Signer::Account(account(2)));
                assert_eq!(signers.len(), 3);
                assert!(
                    signers.contains(&Signer::Account(account(1)))
                        && signers.contains(&Signer::Account(account(3)))
                );
                assert_eq!(block_height, DEFAULT_BLOCK_HEIGHT);
                assert_eq!(deposit_ids, vec![1, 2, 0]);
                assert_eq!(
                    transfers_to_minter_address(&message),
                    vec![
                        (deposit_address(account(2)), 50_000_000 - transaction_fee),
                        (deposit_address(account(3)), 40_000_000),
                        (deposit_address(account(1)), 30_000_000),
                    ]
                );
            })
        })
        .assert_no_more_events();
    for (deposit_id, _, _) in deposits {
        assert_eq!(
            deposit_status(deposit_id),
            DepositSolStatus::Swept {
                signature: fee_payer_signature.into()
            }
        );
    }
    assert_eq!(runtime.set_timer_call_count(), 0);
}

#[tokio::test]
async fn should_record_event_even_if_transaction_submission_fails() {
    setup();
    queue_deposit(0, account(1), MINIMUM_DEPOSIT_AMOUNT);
    let fee_payer_signature = account_signature(&account(1));
    let runtime = TestCanisterRuntime::new()
        .with_increasing_time()
        .add_recent_block(Ok(SLOT))
        .add_stub_response(SendTransactionResult::Inconsistent(vec![]))
        .add_signer(sign_for(&account(1)));

    sweep_queued_deposits(runtime).await;

    EventsAssert::from_recorded()
        .expect_event(|e| assert_matches!(e, EventType::QueuedDeposit { .. }))
        .expect_event(|e| {
            assert_matches!(e, EventType::SubmittedTransaction {
                signature,
                purpose: TransactionPurpose::SweepDeposits { deposit_ids },
                ..
            } if signature == fee_payer_signature && deposit_ids == vec![0])
        })
        .assert_no_more_events();
    assert_eq!(
        deposit_status(0),
        DepositSolStatus::Swept {
            signature: fee_payer_signature.into()
        }
    );
}

#[tokio::test]
async fn should_split_deposits_into_batches_of_max_size() {
    const NUM_DEPOSITS: usize = MAX_DEPOSITS_PER_SWEEP + 2;
    setup();
    queue_deposits_with_increasing_amounts(NUM_DEPOSITS);
    let fee_payer_1 = account(MAX_DEPOSITS_PER_SWEEP - 1);
    let fee_payer_2 = account(NUM_DEPOSITS - 1);
    let fee_payer_signature_1 = account_signature(&fee_payer_1);
    let fee_payer_signature_2 = account_signature(&fee_payer_2);
    let runtime = TestCanisterRuntime::new()
        .with_increasing_time()
        .add_recent_block(Ok(SLOT))
        .transaction_builder(fee_payer_1, fee_payer_signature_1)
        .add_signers((0..MAX_DEPOSITS_PER_SWEEP - 1).map(account))
        .build()
        .transaction_builder(fee_payer_2, fee_payer_signature_2)
        .add_signers([account(MAX_DEPOSITS_PER_SWEEP)])
        .build();

    sweep_queued_deposits(runtime.clone()).await;

    let batch_1_ids: Vec<DepositSolId> = (0..MAX_DEPOSITS_PER_SWEEP as u64).rev().collect();
    let batch_2_ids: Vec<DepositSolId> = (MAX_DEPOSITS_PER_SWEEP as u64..NUM_DEPOSITS as u64)
        .rev()
        .collect();
    let mut events_assert = EventsAssert::from_recorded();
    for _ in 0..NUM_DEPOSITS {
        events_assert =
            events_assert.expect_event(|e| assert_matches!(e, EventType::QueuedDeposit { .. }));
    }
    events_assert
        .expect_event(|e| {
            assert_matches!(e, EventType::SubmittedTransaction {
                signature,
                purpose: TransactionPurpose::SweepDeposits { deposit_ids },
                ..
            } if signature == fee_payer_signature_1 && deposit_ids == batch_1_ids)
        })
        .expect_event(|e| {
            assert_matches!(e, EventType::SubmittedTransaction {
                signature,
                purpose: TransactionPurpose::SweepDeposits { deposit_ids },
                ..
            } if signature == fee_payer_signature_2 && deposit_ids == batch_2_ids)
        })
        .assert_no_more_events();
    assert!(read_state(|s| s.deposits().queued().is_empty()));
    assert_eq!(runtime.set_timer_call_count(), 0);
}

#[tokio::test]
async fn should_reschedule_until_all_deposits_swept() {
    setup();
    let num_deposits = MAX_DEPOSITS_PER_SWEEP * MAX_CONCURRENT_RPC_CALLS + 1;
    queue_deposits_with_increasing_amounts(num_deposits);
    let swept_in_first_round = num_deposits - 1;
    let mut runtime = TestCanisterRuntime::new()
        .with_increasing_time()
        .add_recent_block(Ok(SLOT));
    for batch in 0..MAX_CONCURRENT_RPC_CALLS {
        let first_in_batch = batch * MAX_DEPOSITS_PER_SWEEP;
        let fee_payer_index = first_in_batch + MAX_DEPOSITS_PER_SWEEP - 1;
        let fee_payer = account(fee_payer_index);
        runtime = runtime
            .transaction_builder(fee_payer, account_signature(&fee_payer))
            .add_signers((first_in_batch..fee_payer_index).map(account))
            .build();
    }

    sweep_queued_deposits(runtime.clone()).await;

    read_state(|s| {
        assert_eq!(s.submitted_transactions().len(), MAX_CONCURRENT_RPC_CALLS);
        assert_eq!(s.deposits().queued().len(), 1);
        assert_eq!(s.deposits().swept().deposit_count(), swept_in_first_round);
    });
    assert_eq!(runtime.set_timer_call_count(), 1);

    let last_account = account(num_deposits - 1);
    let runtime = TestCanisterRuntime::new()
        .with_increasing_time()
        .add_recent_block(Ok(SLOT))
        .transaction_builder(last_account, account_signature(&last_account))
        .build();

    sweep_queued_deposits(runtime.clone()).await;

    read_state(|s| {
        assert_eq!(
            s.submitted_transactions().len(),
            MAX_CONCURRENT_RPC_CALLS + 1
        );
        assert!(s.deposits().queued().is_empty());
        assert_eq!(s.deposits().swept().deposit_count(), num_deposits);
    });
    assert_eq!(runtime.set_timer_call_count(), 0);
}

#[tokio::test]
async fn should_fetch_the_master_key_once_for_all_batches_of_a_round() {
    const NUM_DEPOSITS: usize = MAX_DEPOSITS_PER_SWEEP + 1;
    const NUM_BATCHES: usize = 2;
    init_state();
    queue_deposits_with_increasing_amounts(NUM_DEPOSITS);
    let fee_payer_1 = account(MAX_DEPOSITS_PER_SWEEP - 1);
    let fee_payer_2 = account(NUM_DEPOSITS - 1);
    let runtime = TestCanisterRuntime::new()
        .with_increasing_time()
        .add_recent_block(Ok(SLOT))
        .transaction_builder(fee_payer_1, account_signature(&fee_payer_1))
        .add_signers((0..MAX_DEPOSITS_PER_SWEEP - 1).map(account))
        .build()
        .transaction_builder(fee_payer_2, account_signature(&fee_payer_2))
        .build()
        .with_schnorr_public_key(schnorr_master_key_response());

    sweep_queued_deposits(runtime.clone()).await;

    assert_eq!(runtime.schnorr_public_key_call_count(), 1);
    read_state(|s| {
        assert_eq!(s.submitted_transactions().len(), NUM_BATCHES);
        assert_eq!(s.deposits().swept().deposit_count(), NUM_DEPOSITS);
        assert!(s.deposits().queued().is_empty());
    });
}

fn setup() {
    init_state();
    init_schnorr_master_key();
}

fn queue_deposits_with_increasing_amounts(num_deposits: usize) {
    for i in 0..num_deposits {
        queue_deposit(
            i as DepositSolId,
            account(i),
            MINIMUM_DEPOSIT_AMOUNT + i as Lamport,
        );
    }
}

fn deposit_status(deposit_id: DepositSolId) -> DepositSolStatus {
    read_state(|s| s.deposits().status(deposit_id))
}

fn deposit_address(account: Account) -> Address {
    let master_key = read_state(|s| s.minter_public_key().cloned()).unwrap();
    account_address(&master_key, &account)
}

fn transfers_to_minter_address(message: &VersionedMessage) -> Vec<(Address, Lamport)> {
    let VersionedMessage::Legacy(message) = message;
    message
        .instructions
        .iter()
        .map(|instruction| {
            let source = message.account_keys[instruction.accounts[0] as usize];
            let target = message.account_keys[instruction.accounts[1] as usize];
            assert_eq!(target, MINTER_ADDRESS);
            let lamports = assert_matches!(
                bincode::deserialize(&instruction.data),
                Ok(SystemInstruction::Transfer { lamports }) => lamports
            );
            (source, lamports)
        })
        .collect()
}
