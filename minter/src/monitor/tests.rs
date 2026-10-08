use super::{
    MAX_BLOCKHASH_AGE_IN_BLOCKS, MAX_SIGNATURES_PER_STATUS_CHECK, MIN_REBROADCAST_AGE,
    finalize_transactions,
};
use crate::{
    constants::MAX_CONCURRENT_RPC_CALLS,
    rpc::BlockHeight,
    state::{
        MinterTransaction, TaskType,
        event::{EventType, VersionedMessage},
        mutate_state, read_state, reset_state,
    },
    storage::reset_events,
    test_fixtures::{
        EventsAssert, GetTransactionResult, MINIMUM_WITHDRAWAL_AMOUNT, account,
        confirmed_block_at_height, events, finalized_status, init_balance, init_schnorr_master_key,
        init_state, runtime::TestCanisterRuntime, signature,
    },
};
use sol_rpc_types::{
    ConfirmedBlock, MultiRpcResult, RpcError, Slot, TransactionConfirmationStatus,
    TransactionError, TransactionStatus,
};
use solana_transaction::Transaction;

type SignatureStatusesResult = MultiRpcResult<Vec<Option<TransactionStatus>>>;
type SendTransactionResult = MultiRpcResult<sol_rpc_types::Signature>;

const CURRENT_SLOT: Slot = 408_807_102;
const SUBMISSION_SLOT: Slot = CURRENT_SLOT - 10;
const CURRENT_BLOCK_HEIGHT: BlockHeight = BlockHeight::new(CURRENT_SLOT - 1_000);
const OLDEST_VALID_BLOCK_HEIGHT: BlockHeight =
    BlockHeight::new(CURRENT_BLOCK_HEIGHT.get() - MAX_BLOCKHASH_AGE_IN_BLOCKS.get());
const EXPIRED_BLOCK_HEIGHT: BlockHeight = BlockHeight::new(OLDEST_VALID_BLOCK_HEIGHT.get() - 1);

mod finalization {
    use super::*;

    #[tokio::test]
    async fn should_return_early_if_no_submitted_transactions() {
        setup();
        let events_before = EventsAssert::from_recorded();

        finalize_transactions(TestCanisterRuntime::new().with_increasing_time()).await;

        assert_eq!(EventsAssert::from_recorded(), events_before);
    }

    #[tokio::test]
    async fn should_return_early_if_task_already_active() {
        setup();
        submit_sweep_transaction(CURRENT_BLOCK_HEIGHT);

        mutate_state(|s| {
            s.active_tasks_mut().insert(TaskType::FinalizeTransactions);
        });

        let events_before = EventsAssert::from_recorded();

        finalize_transactions(TestCanisterRuntime::new()).await;

        let events_after = EventsAssert::from_recorded();
        assert_eq!(events_before, events_after);
    }

    #[tokio::test]
    async fn should_finalize_but_not_expire_transactions_if_fetching_current_block_fails() {
        setup();
        let finalized = submit_withdrawal_transaction_with_signature(1);
        let not_found = submit_sweep_transaction_with_signature(2, EXPIRED_BLOCK_HEIGHT);

        let runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .add_recent_block(Err(RpcError::ValidationError("Error".to_string())))
            .expect_get_signature_statuses(
                vec![finalized, not_found],
                SignatureStatusesResult::Consistent(Ok(vec![Some(finalized_status()), None])),
            );

        finalize_transactions(runtime).await;

        let events = EventsAssert::from_recorded().expect_contains_event_eq(
            EventType::SucceededTransaction {
                signature: finalized,
            },
        );
        assert!(!events.contains_event(&EventType::ExpiredTransaction {
            signature: not_found
        }));
        read_state(|s| assert!(s.submitted_transactions().contains_key(&not_found)));
    }

    #[tokio::test]
    async fn should_reschedule_until_all_transactions_finalized() {
        setup();

        let num = MAX_CONCURRENT_RPC_CALLS * MAX_SIGNATURES_PER_STATUS_CHECK + 1;
        for i in 0..num {
            submit_withdrawal_transaction_with_signature(i);
        }

        // Round 1: finalizes MAX_CONCURRENT_RPC_CALLS batches, 1 transaction unchecked → reschedule
        let batches = status_check_batches(num);
        let mut runtime = TestCanisterRuntime::new().with_increasing_time();
        for batch in batches.iter().take(MAX_CONCURRENT_RPC_CALLS) {
            runtime = runtime.expect_get_signature_statuses(
                batch.clone(),
                SignatureStatusesResult::Consistent(Ok(vec![
                    Some(finalized_status());
                    batch.len()
                ])),
            );
        }

        finalize_transactions(runtime.clone()).await;

        assert_eq!(read_state(|s| s.submitted_transactions().len()), 1);
        assert_eq!(runtime.set_timer_call_count(), 1);

        // Round 2: finalizes the remaining 1 transaction → no reschedule
        let unchecked_batch = batches.last().expect("BUG: no unchecked batch").clone();
        let runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .expect_get_signature_statuses(
                unchecked_batch,
                SignatureStatusesResult::Consistent(Ok(vec![Some(finalized_status())])),
            );

        finalize_transactions(runtime.clone()).await;

        assert!(read_state(|s| s.submitted_transactions().is_empty()));
        assert_eq!(runtime.set_timer_call_count(), 0);
    }

    #[tokio::test]
    async fn should_finalize_transaction_with_finalized_status() {
        setup();

        let signature = submit_sweep_transaction(CURRENT_BLOCK_HEIGHT);

        let runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .expect_recent_block(CURRENT_SLOT, current_block())
            .expect_get_signature_statuses(
                vec![signature],
                SignatureStatusesResult::Consistent(Ok(vec![Some(finalized_status())])),
            )
            .expect_get_transaction(signature, GetTransactionResult::Consistent(Ok(None)));

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
        for status in [processed_status(), confirmed_status()] {
            for block_height in [CURRENT_BLOCK_HEIGHT, EXPIRED_BLOCK_HEIGHT] {
                should_not_finalize(block_height, Some(status.clone())).await;
            }
        }
        should_not_finalize(CURRENT_BLOCK_HEIGHT, None).await;
    }

    async fn should_not_finalize(block_height: BlockHeight, status: Option<TransactionStatus>) {
        reset_state();
        reset_events();
        setup();

        let signature = submit_sweep_transaction(block_height);

        let runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .expect_recent_block(CURRENT_SLOT, current_block())
            .expect_get_signature_statuses(
                vec![signature],
                SignatureStatusesResult::Consistent(Ok(vec![status])),
            );

        let events_before = EventsAssert::from_recorded();

        finalize_transactions(runtime).await;

        let events_after = EventsAssert::from_recorded();
        assert_eq!(events_before, events_after);

        read_state(|s| assert_eq!(s.submitted_transactions().len(), 1));
    }

    #[tokio::test]
    async fn should_finalize_an_in_flight_withdrawal_without_fetching_a_block() {
        setup();
        let signature = submit_withdrawal_transaction();

        let runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .expect_get_signature_statuses(
                vec![signature],
                SignatureStatusesResult::Consistent(Ok(vec![Some(finalized_status())])),
            );

        finalize_transactions(runtime).await;

        EventsAssert::from_recorded()
            .expect_contains_event_eq(EventType::SucceededTransaction { signature });
        read_state(|s| {
            assert!(s.submitted_transactions().is_empty());
            assert!(s.succeeded_transactions().contains(&signature));
        });
    }

    #[tokio::test]
    async fn should_rebroadcast_a_missing_withdrawal_unchanged() {
        setup();
        let signature = submit_withdrawal_transaction();
        let events_before = EventsAssert::from_recorded();

        let runtime = TestCanisterRuntime::new()
            .with_increasing_time_from(min_rebroadcast_age_nanos())
            .expect_get_signature_statuses(
                vec![signature],
                SignatureStatusesResult::Consistent(Ok(vec![None])),
            )
            .expect_send_exact_transaction_skipping_preflight(
                submitted_transaction(&signature),
                SendTransactionResult::Consistent(Ok(signature.into())),
            );

        finalize_transactions(runtime).await;

        assert_eq!(EventsAssert::from_recorded(), events_before);
        read_state(|s| assert!(s.submitted_transactions().contains_key(&signature)));
    }

    #[tokio::test]
    async fn should_rebroadcast_a_missing_withdrawal_only_after_the_minimum_age() {
        setup();
        let signature = submit_withdrawal_transaction();

        let too_early = TestCanisterRuntime::new()
            .with_increasing_time()
            .expect_get_signature_statuses(
                vec![signature],
                SignatureStatusesResult::Consistent(Ok(vec![None])),
            );

        finalize_transactions(too_early).await;

        let old_enough = TestCanisterRuntime::new()
            .with_increasing_time_from(min_rebroadcast_age_nanos())
            .expect_get_signature_statuses(
                vec![signature],
                SignatureStatusesResult::Consistent(Ok(vec![None])),
            )
            .expect_send_transaction_skipping_preflight(
                signature,
                SendTransactionResult::Consistent(Ok(signature.into())),
            );

        finalize_transactions(old_enough).await;

        read_state(|s| assert!(s.submitted_transactions().contains_key(&signature)));
    }

    fn min_rebroadcast_age_nanos() -> u64 {
        MIN_REBROADCAST_AGE.as_nanos() as u64
    }

    fn submitted_transaction(signature: &solana_signature::Signature) -> Transaction {
        let message = read_state(|s| {
            match s
                .submitted_transactions()
                .get(signature)
                .expect("the transaction is submitted")
            {
                MinterTransaction::Withdrawal {
                    message: VersionedMessage::Legacy(message),
                    ..
                } => message.clone(),
                other => panic!("expected a withdrawal transaction, got {other:?}"),
            }
        });
        Transaction {
            signatures: vec![*signature],
            message,
        }
    }

    fn submit_withdrawal_transaction() -> solana_signature::Signature {
        submit_withdrawal_transaction_with_signature(0x77)
    }

    fn submit_withdrawal_transaction_with_signature(i: usize) -> solana_signature::Signature {
        let signature = signature(i);
        events::accept_withdrawal(account(i), i as u64, MINIMUM_WITHDRAWAL_AMOUNT);
        events::submit_withdrawal(signature, vec![i as u64]);
        signature
    }

    #[tokio::test]
    async fn should_record_failed_transaction_event_on_error() {
        setup();

        let signature = submit_sweep_transaction(CURRENT_BLOCK_HEIGHT);

        let runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .expect_recent_block(CURRENT_SLOT, current_block())
            .expect_get_signature_statuses(
                vec![signature],
                SignatureStatusesResult::Consistent(Ok(vec![Some(TransactionStatus {
                    slot: SUBMISSION_SLOT,
                    status: Err(TransactionError::InsufficientFundsForFee),
                    err: Some(TransactionError::InsufficientFundsForFee),
                    confirmation_status: Some(TransactionConfirmationStatus::Finalized),
                })])),
            );

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
        submit_sweep_transaction_with_signature(sig_a, CURRENT_BLOCK_HEIGHT);
        submit_sweep_transaction_with_signature(sig_b, CURRENT_BLOCK_HEIGHT);
        submit_sweep_transaction_with_signature(sig_c, CURRENT_BLOCK_HEIGHT);

        let runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .expect_recent_block(CURRENT_SLOT, current_block())
            .expect_get_signature_statuses(
                vec![signature(sig_a), signature(sig_b), signature(sig_c)],
                SignatureStatusesResult::Consistent(Ok(vec![
                    Some(finalized_status()),
                    None,
                    Some(finalized_status()),
                ])),
            )
            .expect_get_transaction(signature(sig_a), GetTransactionResult::Consistent(Ok(None)))
            .expect_get_transaction(signature(sig_c), GetTransactionResult::Consistent(Ok(None)));

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
        });
    }

    #[tokio::test]
    async fn should_never_expire_nonce_withdrawal_with_missing_status() {
        setup();
        let sweep = submit_sweep_transaction_with_signature(1, EXPIRED_BLOCK_HEIGHT);
        let nonce_withdrawal = submit_withdrawal_transaction_with_signature(2);

        let runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .expect_recent_block(CURRENT_SLOT, current_block())
            .expect_get_signature_statuses(
                vec![sweep, nonce_withdrawal],
                SignatureStatusesResult::Consistent(Ok(vec![None, None])),
            );

        finalize_transactions(runtime).await;

        let events = EventsAssert::from_recorded()
            .expect_contains_event_eq(EventType::ExpiredTransaction { signature: sweep });
        assert!(!events.contains_event(&EventType::ExpiredTransaction {
            signature: nonce_withdrawal
        }));
        read_state(|s| assert!(s.submitted_transactions().contains_key(&nonce_withdrawal)));
    }

    #[tokio::test]
    async fn should_not_expire_transaction_if_status_check_fails() {
        setup();

        let signature = submit_sweep_transaction(EXPIRED_BLOCK_HEIGHT);

        let events_before = EventsAssert::from_recorded();

        let finalize_runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .expect_recent_block(CURRENT_SLOT, current_block())
            .expect_get_signature_statuses(
                vec![signature],
                SignatureStatusesResult::Consistent(Err(RpcError::ValidationError(
                    "Error".to_string(),
                ))),
            );

        finalize_transactions(finalize_runtime).await;

        let events_after = EventsAssert::from_recorded();
        assert_eq!(events_before, events_after);

        read_state(|s| {
            assert_eq!(s.submitted_transactions().len(), 1);
        });
    }

    struct ExpiryCase {
        name: &'static str,
        transaction_block_height: BlockHeight,
        should_expire: bool,
    }

    #[tokio::test]
    async fn should_expire_transaction_past_the_blockhash_age_limit() {
        let cases = [
            ExpiryCase {
                name: "at the age limit",
                transaction_block_height: OLDEST_VALID_BLOCK_HEIGHT,
                should_expire: false,
            },
            ExpiryCase {
                name: "past the age limit",
                transaction_block_height: BlockHeight::new(OLDEST_VALID_BLOCK_HEIGHT.get() - 1),
                should_expire: true,
            },
            ExpiryCase {
                name: "ahead of the current block height",
                transaction_block_height: BlockHeight::new(u64::MAX),
                should_expire: false,
            },
        ];

        for case in cases {
            reset_state();
            reset_events();
            setup();
            let signature = submit_sweep_transaction(case.transaction_block_height);
            let runtime = TestCanisterRuntime::new()
                .with_increasing_time()
                .expect_recent_block(CURRENT_SLOT, current_block())
                .expect_get_signature_statuses(
                    vec![signature],
                    SignatureStatusesResult::Consistent(Ok(vec![None])),
                );

            finalize_transactions(runtime).await;

            let expired = EventsAssert::from_recorded()
                .contains_event(&EventType::ExpiredTransaction { signature });
            assert_eq!(expired, case.should_expire, "{}", case.name);
            read_state(|s| {
                assert_eq!(
                    s.submitted_transactions().contains_key(&signature),
                    !case.should_expire,
                    "{}",
                    case.name
                );
            });
        }
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
}

fn setup() {
    init_state();
    init_balance();
    init_schnorr_master_key();
}

fn current_block() -> ConfirmedBlock {
    confirmed_block_at_height(CURRENT_BLOCK_HEIGHT)
}

/// The signatures of `num_transactions` submitted transactions, batched as
/// `finalize_transactions` checks their statuses: ordered as the state stores them, in
/// chunks of [`MAX_SIGNATURES_PER_STATUS_CHECK`].
fn status_check_batches(num_transactions: usize) -> Vec<Vec<solana_signature::Signature>> {
    let mut signatures: Vec<_> = (0..num_transactions).map(signature).collect();
    signatures.sort_unstable();
    signatures
        .chunks(MAX_SIGNATURES_PER_STATUS_CHECK)
        .map(<[solana_signature::Signature]>::to_vec)
        .collect()
}

fn submit_sweep_transaction(block_height: BlockHeight) -> solana_signature::Signature {
    submit_sweep_transaction_with_signature(1, block_height)
}

fn submit_sweep_transaction_with_signature(
    i: usize,
    block_height: BlockHeight,
) -> solana_signature::Signature {
    let signature = signature(i);
    let deposit_id = crate::state::read_state(|state| state.deposits().next_id());
    events::queue_deposit(deposit_id, account(i), 1_000_000);
    events::submit_sweep_at_height(signature, vec![deposit_id], block_height);
    signature
}
