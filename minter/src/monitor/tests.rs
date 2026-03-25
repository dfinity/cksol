use super::{MAX_BLOCKHASH_AGE, monitor_submitted_transactions};
use crate::{
    address::derive_public_key,
    state::{TaskType, audit::process_event, event::EventType, mutate_state, read_state},
    test_fixtures::{
        EventsAssert, MINTER_ACCOUNT, init_schnorr_master_key, init_state,
        runtime::TestCanisterRuntime,
    },
};
use assert_matches::assert_matches;
use ic_ed25519::{PocketIcMasterPublicKeyId, PublicKey};
use sol_rpc_types::{
    ConfirmedBlock, MultiRpcResult, RpcError, Slot, TransactionConfirmationStatus,
    TransactionError, TransactionStatus,
};
use solana_hash::Hash;
use solana_message::Message;
use solana_signature::Signature;

type SlotResult = MultiRpcResult<Slot>;
type BlockResult = MultiRpcResult<ConfirmedBlock>;
type SendTransactionResult = MultiRpcResult<sol_rpc_types::Signature>;
type SignatureStatusesResult = MultiRpcResult<Vec<Option<TransactionStatus>>>;

#[tokio::test]
async fn should_return_early_if_no_submitted_transactions() {
    setup();

    monitor_submitted_transactions(TestCanisterRuntime::new().with_increasing_time()).await;

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

    EventsAssert::from_recorded()
        .expect_event(|e| assert_matches!(e, EventType::SubmittedTransaction { .. }))
        .assert_no_more_events();
}

mod finalization {
    use super::*;

    #[tokio::test]
    async fn should_finalize_transaction_with_finalized_status() {
        setup();

        let signature = Signature::from([0x01; 64]);
        add_submitted_transaction(signature, 100);

        let runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .add_stub_response(SignatureStatusesResult::Consistent(Ok(vec![Some(
                finalized_status(100),
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
    async fn should_finalize_transaction_even_after_blockhash_expired() {
        setup();

        let signature = Signature::from([0x01; 64]);
        add_submitted_transaction(signature, 10);

        let runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .add_stub_response(SignatureStatusesResult::Consistent(Ok(vec![Some(
                finalized_status(10),
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
    async fn should_not_finalize_confirmed_transaction_with_recent_slot() {
        should_not_finalize(
            100,
            Some(TransactionStatus {
                slot: 100,
                status: Ok(()),
                err: None,
                confirmation_status: Some(TransactionConfirmationStatus::Confirmed),
            }),
            None,
        )
        .await;
    }

    #[tokio::test]
    async fn should_not_finalize_confirmed_transaction_with_expired_slot() {
        should_not_finalize(
            10,
            Some(TransactionStatus {
                slot: 10,
                status: Ok(()),
                err: None,
                confirmation_status: Some(TransactionConfirmationStatus::Confirmed),
            }),
            None,
        )
        .await;
    }

    #[tokio::test]
    async fn should_not_finalize_processed_transaction() {
        should_not_finalize(
            100,
            Some(TransactionStatus {
                slot: 100,
                status: Ok(()),
                err: None,
                confirmation_status: Some(TransactionConfirmationStatus::Processed),
            }),
            None,
        )
        .await;
    }

    #[tokio::test]
    async fn should_not_finalize_transaction_with_no_status() {
        should_not_finalize(
            100,
            None,
            Some(SlotResult::Consistent(Ok(100 + MAX_BLOCKHASH_AGE - 1))),
        )
        .await;
    }

    async fn should_not_finalize(
        slot: Slot,
        status: Option<TransactionStatus>,
        get_slot_response: Option<SlotResult>,
    ) {
        setup();

        add_submitted_transaction(Signature::from([0x01; 64]), slot);

        let mut runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .add_stub_response(SignatureStatusesResult::Consistent(Ok(vec![status])));

        if let Some(slot_response) = get_slot_response {
            runtime = runtime.add_stub_response(slot_response);
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

        let signature = Signature::from([0x01; 64]);
        add_submitted_transaction(signature, 100);

        let runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .add_stub_response(SignatureStatusesResult::Consistent(Ok(vec![Some(
                TransactionStatus {
                    slot: 100,
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

        let sig_a = Signature::from([0x01; 64]);
        let sig_b = Signature::from([0x02; 64]);
        let sig_c = Signature::from([0x03; 64]);
        add_submitted_transaction(sig_a, 100);
        add_submitted_transaction(sig_b, 100);
        add_submitted_transaction(sig_c, 100);

        let runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .add_stub_response(SignatureStatusesResult::Consistent(Ok(vec![
                Some(finalized_status(100)),
                None,
                Some(finalized_status(100)),
            ])))
            // getSlot for the one not_found transaction
            .add_stub_response(SlotResult::Consistent(Ok(100)));

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

        let old_signature = Signature::from([0x01; 64]);
        let original_slot = 10;
        add_submitted_transaction(old_signature, original_slot);

        let current_slot = original_slot + MAX_BLOCKHASH_AGE + 1;
        let new_slot = current_slot + 5;
        let new_signature = Signature::from([0xAA; 64]);

        let runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            // getSignatureStatuses: not found
            .add_stub_response(SignatureStatusesResult::Consistent(Ok(vec![None])))
            // getSlot for expiry check
            .add_stub_response(SlotResult::Consistent(Ok(current_slot)))
            // get_recent_blockhash (getSlot + getBlock)
            .add_stub_response(SlotResult::Consistent(Ok(new_slot)))
            .add_stub_response(BlockResult::Consistent(Ok(block())))
            // get_slot for new slot
            .add_stub_response(SlotResult::Consistent(Ok(new_slot)))
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
                    } if old_sig == old_signature && new_sig == new_signature && slot == new_slot
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

        add_submitted_transaction(Signature::from([0x01; 64]), 10);

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

        let old_signature = Signature::from([0x01; 64]);
        let original_slot = 10;
        add_submitted_transaction(old_signature, original_slot);

        let current_slot = original_slot + MAX_BLOCKHASH_AGE + 1;
        let new_slot = current_slot + 5;
        let new_signature = Signature::from([0xAA; 64]);

        let runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .add_stub_response(SignatureStatusesResult::Consistent(Ok(vec![None])))
            .add_stub_response(SlotResult::Consistent(Ok(current_slot)))
            .add_stub_response(SlotResult::Consistent(Ok(new_slot)))
            .add_stub_response(BlockResult::Consistent(Ok(block())))
            .add_stub_response(SlotResult::Consistent(Ok(new_slot)))
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
                    } if old_sig == old_signature && new_sig == new_signature && slot == new_slot
                )
            })
            .assert_no_more_events();
    }

    #[tokio::test]
    async fn should_resubmit_multiple_expired_transactions_in_batches() {
        use crate::constants::MAX_CONCURRENT_RPC_CALLS;

        setup();

        let num_transactions = MAX_CONCURRENT_RPC_CALLS + 2; // 12 → 2 rounds
        let original_slot = 10;
        for i in 0..num_transactions {
            add_submitted_transaction(Signature::from([i as u8; 64]), original_slot);
        }

        let current_slot = original_slot + MAX_BLOCKHASH_AGE + 1;
        let new_slot = current_slot + 5;

        let mut runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            // getSignatureStatuses: all not found
            .add_stub_response(SignatureStatusesResult::Consistent(Ok(
                vec![None; num_transactions],
            )))
            // getSlot for expiry check
            .add_stub_response(SlotResult::Consistent(Ok(current_slot)))
            // Round 1: get_recent_blockhash + get_slot
            .add_stub_response(SlotResult::Consistent(Ok(new_slot)))
            .add_stub_response(BlockResult::Consistent(Ok(block())))
            .add_stub_response(SlotResult::Consistent(Ok(new_slot)));

        for i in 0..MAX_CONCURRENT_RPC_CALLS {
            runtime = runtime
                .add_stub_response(SendTransactionResult::Consistent(Ok(Signature::from(
                    [0xA0 + i as u8; 64],
                )
                .into())))
                .add_signature([0xA0 + i as u8; 64]);
        }

        // Round 2: get_recent_blockhash + get_slot
        runtime = runtime
            .add_stub_response(SlotResult::Consistent(Ok(new_slot)))
            .add_stub_response(BlockResult::Consistent(Ok(block())))
            .add_stub_response(SlotResult::Consistent(Ok(new_slot)));

        for i in 0..2_usize {
            runtime = runtime
                .add_stub_response(SendTransactionResult::Consistent(Ok(Signature::from(
                    [0xB0 + i as u8; 64],
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

fn add_submitted_transaction(signature: Signature, slot: Slot) {
    let message = Message::new_with_blockhash(&[], Some(&minter_address()), &Hash::default());
    mutate_state(|state| {
        process_event(
            state,
            EventType::SubmittedTransaction {
                signature,
                transaction: message,
                signers: vec![MINTER_ACCOUNT],
                slot,
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
