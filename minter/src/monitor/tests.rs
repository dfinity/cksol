use super::{MAX_BLOCKHASH_AGE, monitor_submitted_transactions};
use crate::{
    address::derive_public_key,
    numeric::LedgerBurnIndex,
    state::{
        TaskType,
        audit::process_event,
        event::{EventType, WithdrawSolRequest},
        mutate_state, read_state,
    },
    test_fixtures::{
        EventsAssert, MINTER_ACCOUNT, init_schnorr_master_key, init_state,
        runtime::TestCanisterRuntime,
    },
    withdraw_sol::withdraw_sol_status,
};
use assert_matches::assert_matches;
use cksol_types::WithdrawSolStatus;
use ic_ed25519::{PocketIcMasterPublicKeyId, PublicKey};
use sol_rpc_types::{ConfirmedBlock, MultiRpcResult, RpcError, Slot};
use solana_hash::Hash;
use solana_message::Message;
use solana_signature::Signature;

type SlotResult = MultiRpcResult<Slot>;
type BlockResult = MultiRpcResult<ConfirmedBlock>;
type SendTransactionResult = MultiRpcResult<sol_rpc_types::Signature>;

#[tokio::test]
async fn should_return_early_if_no_transactions_to_resubmit() {
    setup();

    let runtime = TestCanisterRuntime::new().with_increasing_time();

    monitor_submitted_transactions(runtime).await;

    EventsAssert::assert_no_events_recorded();
}

#[tokio::test]
async fn should_return_early_if_task_already_active() {
    setup();
    add_submitted_transaction(Signature::from([0x01; 64]), 10);

    mutate_state(|s| {
        s.active_tasks_mut()
            .insert(TaskType::MonitorSubmittedTransactions);
    });

    monitor_submitted_transactions(TestCanisterRuntime::new()).await;

    // Only SubmittedTransaction event from setup, no resubmission events
    EventsAssert::from_recorded()
        .expect_event(|e| assert_matches!(e, EventType::SubmittedTransaction { .. }))
        .assert_no_more_events();
}

#[tokio::test]
async fn should_return_early_if_fetching_current_slot_fails() {
    setup();
    add_submitted_transaction(Signature::from([0x01; 64]), 10);

    let error = SlotResult::Consistent(Err(RpcError::ValidationError("Error".to_string())));
    let runtime = TestCanisterRuntime::new()
        .add_stub_response(error.clone())
        .add_stub_response(error.clone())
        .add_stub_response(error);

    monitor_submitted_transactions(runtime).await;

    // Only SubmittedTransaction event from setup, no resubmission events
    EventsAssert::from_recorded()
        .expect_event(|e| assert_matches!(e, EventType::SubmittedTransaction { .. }))
        .assert_no_more_events();
}

#[tokio::test]
async fn should_not_resubmit_if_transaction_not_expired() {
    setup();

    let original_slot = 100;
    add_submitted_transaction(Signature::from([0x01; 64]), original_slot);

    // Current slot is within MAX_BLOCKHASH_AGE of original slot
    let current_slot = original_slot + MAX_BLOCKHASH_AGE - 1;
    let runtime = TestCanisterRuntime::new()
        .with_increasing_time()
        .add_stub_response(SlotResult::Consistent(Ok(current_slot)));

    monitor_submitted_transactions(runtime).await;

    // Only SubmittedTransaction event from setup, no resubmission
    EventsAssert::from_recorded()
        .expect_event(|e| assert_matches!(e, EventType::SubmittedTransaction { .. }))
        .assert_no_more_events();

    // Transaction should still be in submitted_transactions
    read_state(|s| {
        assert_eq!(s.submitted_transactions().len(), 1);
    });
}

#[tokio::test]
async fn should_resubmit_single_expired_transaction() {
    setup();

    let old_signature = Signature::from([0x01; 64]);
    let original_slot = 10;
    add_submitted_transaction(old_signature, original_slot);

    // Current slot is past MAX_BLOCKHASH_AGE
    let current_slot = original_slot + MAX_BLOCKHASH_AGE + 1;
    let new_slot = current_slot + 5;
    let new_signature = Signature::from([0xAA; 64]);

    let runtime = TestCanisterRuntime::new()
        .with_increasing_time()
        // get_slot for current slot check
        .add_stub_response(SlotResult::Consistent(Ok(current_slot)))
        // get_recent_blockhash calls
        .add_stub_response(SlotResult::Consistent(Ok(new_slot)))
        .add_stub_response(BlockResult::Consistent(Ok(block())))
        // get_slot for new slot
        .add_stub_response(SlotResult::Consistent(Ok(new_slot)))
        // submit_transaction
        .add_stub_response(SendTransactionResult::Consistent(Ok(new_signature.into())))
        // Signature for re-signing (only fee payer since message has no other signers)
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
                } if old_sig == old_signature && new_sig == new_signature && slot == new_slot
            )
        })
        .assert_no_more_events();

    // Old transaction should be replaced with new one
    read_state(|s| {
        assert_eq!(s.submitted_transactions().len(), 1);
        assert!(s.submitted_transactions().contains_key(&new_signature));
        assert!(!s.submitted_transactions().contains_key(&old_signature));
    });
}

#[tokio::test]
async fn should_update_withdrawal_status_signature_after_resubmission() {
    setup();

    let old_signature = Signature::from([0x01; 64]);
    let burn_index = LedgerBurnIndex::from(42u64);
    let original_slot = 10;

    // Set up a sent withdrawal linked to the submitted transaction
    let setup_runtime = TestCanisterRuntime::new().with_increasing_time();
    mutate_state(|state| {
        process_event(
            state,
            EventType::AcceptedWithdrawSolRequest(WithdrawSolRequest {
                account: MINTER_ACCOUNT,
                solana_address: [0xAB; 32],
                burn_block_index: burn_index,
                withdrawal_amount: 1_000_000,
                withdrawal_fee: 5_000,
            }),
            &setup_runtime,
        );
    });
    add_withdrawal_submitted_transaction(old_signature, original_slot, vec![burn_index]);

    // Withdrawal status should reference the old signature
    assert_matches!(
        withdraw_sol_status(*burn_index.get()),
        WithdrawSolStatus::TxSent(tx) => {
            assert_eq!(tx.transaction_hash, old_signature.to_string());
        }
    );

    // Resubmit the transaction
    let current_slot = original_slot + MAX_BLOCKHASH_AGE + 1;
    let new_slot = current_slot + 5;
    let new_signature = Signature::from([0xBB; 64]);

    let runtime = TestCanisterRuntime::new()
        .with_increasing_time()
        .add_stub_response(SlotResult::Consistent(Ok(current_slot)))
        .add_stub_response(SlotResult::Consistent(Ok(new_slot)))
        .add_stub_response(BlockResult::Consistent(Ok(block())))
        .add_stub_response(SlotResult::Consistent(Ok(new_slot)))
        .add_stub_response(SendTransactionResult::Consistent(Ok(new_signature.into())))
        .add_signature(new_signature.into());

    monitor_submitted_transactions(runtime).await;

    // Withdrawal status should now reference the new signature
    assert_matches!(
        withdraw_sol_status(*burn_index.get()),
        WithdrawSolStatus::TxSent(tx) => {
            assert_eq!(tx.transaction_hash, new_signature.to_string());
        }
    );
}

#[tokio::test]
async fn should_record_event_even_if_submission_fails() {
    setup();

    let old_signature = Signature::from([0x01; 64]);
    let original_slot = 10;
    add_submitted_transaction(old_signature, original_slot);

    let current_slot = original_slot + MAX_BLOCKHASH_AGE + 1;
    let new_slot = current_slot + 5;
    let new_signature = Signature::from([0xAA; 64]);

    let runtime = TestCanisterRuntime::new()
        .with_increasing_time()
        // get_slot for current slot check
        .add_stub_response(SlotResult::Consistent(Ok(current_slot)))
        // get_recent_blockhash calls
        .add_stub_response(SlotResult::Consistent(Ok(new_slot)))
        .add_stub_response(BlockResult::Consistent(Ok(block())))
        // get_slot for new slot
        .add_stub_response(SlotResult::Consistent(Ok(new_slot)))
        // submit_transaction fails
        .add_stub_response(SendTransactionResult::Inconsistent(vec![]))
        .add_signature(new_signature.into());

    monitor_submitted_transactions(runtime).await;

    // ResubmittedTransaction event should still be recorded
    EventsAssert::from_recorded()
        .expect_event(|e| assert_matches!(e, EventType::SubmittedTransaction { .. }))
        .expect_event(|e| {
            assert_matches!(
                e,
                EventType::ResubmittedTransaction {
                    old_signature: old_sig,
                    new_signature: new_sig,
                    new_slot: slot,
                } if old_sig == old_signature && new_sig == new_signature && slot == new_slot
            )
        })
        .assert_no_more_events();
}

fn setup() {
    init_state();
    init_schnorr_master_key();
}

fn minter_address() -> solana_address::Address {
    use crate::state::SchnorrPublicKey;
    let master_key = SchnorrPublicKey {
        public_key: PublicKey::pocketic_key(PocketIcMasterPublicKeyId::Key1),
        chain_code: [1; 32],
    };
    derive_public_key(&master_key, vec![])
        .serialize_raw()
        .into()
}

fn add_withdrawal_submitted_transaction(
    signature: Signature,
    slot: Slot,
    burn_indices: Vec<LedgerBurnIndex>,
) {
    let message = Message::new_with_blockhash(&[], Some(&minter_address()), &Hash::default());
    mutate_state(|state| {
        process_event(
            state,
            EventType::SubmittedTransaction {
                signature,
                message: message.into(),
                signers: vec![MINTER_ACCOUNT],
                slot,
                purpose: crate::state::event::TransactionPurpose::WithdrawSol { burn_indices },
            },
            &TestCanisterRuntime::new().with_increasing_time(),
        )
    });
}

fn add_submitted_transaction(signature: Signature, slot: Slot) {
    let message = Message::new_with_blockhash(&[], Some(&minter_address()), &Hash::default());
    mutate_state(|state| {
        process_event(
            state,
            EventType::SubmittedTransaction {
                signature,
                message: message.into(),
                signers: vec![MINTER_ACCOUNT],
                slot,
                purpose: crate::state::event::TransactionPurpose::ConsolidateDeposits {
                    mint_indices: vec![],
                },
            },
            &TestCanisterRuntime::new().with_increasing_time(),
        )
    });
}

fn block() -> ConfirmedBlock {
    ConfirmedBlock {
        previous_blockhash: Default::default(),
        blockhash: Hash::from([0x42; 32]).into(),
        parent_slot: 0,
        block_time: None,
        block_height: None,
        signatures: None,
        rewards: None,
        num_reward_partitions: None,
        transactions: None,
    }
}
