use crate::numeric::LedgerBurnIndex;
use crate::sol_transfer::MAX_WITHDRAWALS_PER_TX;
use crate::test_fixtures::EventsAssert;
use crate::{
    guard::{TimerGuard, withdrawal_guard},
    state::{
        TaskType,
        event::{EventType, TransactionPurpose},
    },
    test_fixtures::{
        MINIMUM_WITHDRAWAL_AMOUNT, MINTER_ACCOUNT, WITHDRAWAL_FEE, account, confirmed_block,
        events, init_schnorr_master_key, init_state, runtime::TestCanisterRuntime, signature,
    },
    withdraw::{MAX_WITHDRAWAL_ROUNDS, process_pending_withdrawals, withdraw, withdrawal_status},
};
use assert_matches::assert_matches;
use candid::{Nat, Principal};
use canlog::Log;
use cksol_types::TxFinalizedStatus;
use cksol_types::WithdrawalStatus;
use cksol_types::{WithdrawalError, WithdrawalOk};
use cksol_types_internal::log::Priority;
use ic_canister_runtime::IcError;
use ic_cdk::call::CallRejected;
use ic_cdk_management_canister::SignCallError;
use icrc_ledger_types::{icrc1::account::Account, icrc2::transfer_from::TransferFromError};
use sol_rpc_types::{MultiRpcResult, RpcError, Slot};
use solana_signature::Signature;
const VALID_ADDRESS: &str = "E4MpwNnMWs2XtW5gVrxZvyS7fMq31QD5HvbxmwP45Tz3";

fn test_caller() -> Account {
    Principal::from_slice(&[1_u8; 20]).into()
}

#[tokio::test]
async fn should_return_error_if_calling_ledger_fails() {
    init_state();

    let runtime = TestCanisterRuntime::new().add_stub_error(IcError::CallPerformFailed);

    let result = withdraw(
        &runtime,
        test_caller(),
        MINIMUM_WITHDRAWAL_AMOUNT,
        VALID_ADDRESS.to_string(),
    )
    .await;

    assert_matches!(
        result,
        Err(WithdrawalError::TemporarilyUnavailable(e)) => assert!(e.contains("Failed to burn tokens"))
    );
}

#[tokio::test]
async fn should_return_error_if_ledger_unavailable() {
    init_state();

    let runtime = TestCanisterRuntime::new().add_stub_response(Err::<Nat, TransferFromError>(
        TransferFromError::TemporarilyUnavailable,
    ));

    let result = withdraw(
        &runtime,
        test_caller(),
        MINIMUM_WITHDRAWAL_AMOUNT,
        VALID_ADDRESS.to_string(),
    )
    .await;

    assert_eq!(
        result,
        Err(WithdrawalError::TemporarilyUnavailable(
            "Ledger is temporarily unavailable".to_string(),
        ))
    );
}

#[tokio::test]
async fn should_return_error_if_insufficient_allowance() {
    init_state();

    let runtime = TestCanisterRuntime::new().add_stub_response(Err::<Nat, TransferFromError>(
        TransferFromError::InsufficientAllowance {
            allowance: Nat::from(123u64),
        },
    ));

    let result = withdraw(
        &runtime,
        test_caller(),
        MINIMUM_WITHDRAWAL_AMOUNT,
        VALID_ADDRESS.to_string(),
    )
    .await;

    assert_eq!(
        result,
        Err(WithdrawalError::InsufficientAllowance { allowance: 123u64 })
    );
}

#[tokio::test]
async fn should_return_error_if_insufficient_funds() {
    init_state();

    let runtime = TestCanisterRuntime::new().add_stub_response(Err::<Nat, TransferFromError>(
        TransferFromError::InsufficientFunds {
            balance: Nat::from(123u64),
        },
    ));

    let result = withdraw(
        &runtime,
        test_caller(),
        MINIMUM_WITHDRAWAL_AMOUNT,
        VALID_ADDRESS.to_string(),
    )
    .await;

    assert_eq!(
        result,
        Err(WithdrawalError::InsufficientFunds { balance: 123u64 })
    );
}

#[tokio::test]
async fn should_return_temporarily_unavailable_on_generic_error() {
    init_state();

    let runtime = TestCanisterRuntime::new().add_stub_response(Err::<Nat, TransferFromError>(
        TransferFromError::GenericError {
            error_code: Nat::from(123u64),
            message: "msg".to_string(),
        },
    ));

    let result = withdraw(
        &runtime,
        test_caller(),
        MINIMUM_WITHDRAWAL_AMOUNT,
        VALID_ADDRESS.to_string(),
    )
    .await;

    assert_eq!(
        result,
        Err(WithdrawalError::TemporarilyUnavailable(
            "Ledger returned a generic error: code 123, message: msg".to_string()
        ))
    );
}

#[tokio::test]
async fn should_return_ok_if_burn_succeeds() {
    init_state();

    let runtime = TestCanisterRuntime::new()
        .add_stub_response(Ok::<Nat, TransferFromError>(Nat::from(123u64)))
        .with_increasing_time();

    let result = withdraw(
        &runtime,
        test_caller(),
        MINIMUM_WITHDRAWAL_AMOUNT,
        VALID_ADDRESS.to_string(),
    )
    .await;

    assert_eq!(
        result,
        Ok(WithdrawalOk {
            block_index: 123u64
        })
    );
}

#[tokio::test]
async fn should_return_error_if_address_malformed() {
    init_state();

    let runtime = TestCanisterRuntime::new();

    let result = withdraw(
        &runtime,
        test_caller(),
        MINIMUM_WITHDRAWAL_AMOUNT,
        "not-a-valid-address".to_string(),
    )
    .await;

    assert_matches!(result, Err(WithdrawalError::MalformedAddress(_)));
}

#[tokio::test]
async fn should_return_error_if_amount_too_low() {
    init_state();

    let runtime = TestCanisterRuntime::new();

    let result = withdraw(
        &runtime,
        test_caller(),
        MINIMUM_WITHDRAWAL_AMOUNT - 1,
        VALID_ADDRESS.to_string(),
    )
    .await;

    assert_eq!(
        result,
        Err(WithdrawalError::ValueTooSmall {
            minimum_withdrawal_amount: MINIMUM_WITHDRAWAL_AMOUNT,
            withdrawal_amount: MINIMUM_WITHDRAWAL_AMOUNT - 1,
        })
    );
}

#[tokio::test]
async fn should_return_error_if_already_processing() {
    init_state();

    let from = test_caller();
    let _guard = withdrawal_guard(from).unwrap();

    let runtime = TestCanisterRuntime::new();

    let result = withdraw(
        &runtime,
        from,
        MINIMUM_WITHDRAWAL_AMOUNT,
        VALID_ADDRESS.to_string(),
    )
    .await;

    assert_eq!(result, Err(WithdrawalError::AlreadyProcessing));
}

mod process_pending_withdrawals_tests {
    use crate::constants::MAX_CONCURRENT_RPC_CALLS;

    use super::*;

    type GetSlotResult = MultiRpcResult<Slot>;
    type GetBlockResult = MultiRpcResult<sol_rpc_types::ConfirmedBlock>;
    type SendTransactionResult = MultiRpcResult<sol_rpc_types::Signature>;

    #[tokio::test]
    async fn should_do_nothing_if_no_pending_withdrawals() {
        init_state();

        let runtime = TestCanisterRuntime::new();
        process_pending_withdrawals(&runtime).await;

        EventsAssert::assert_no_events_recorded();
    }

    #[tokio::test]
    async fn should_skip_if_already_processing() {
        init_state();

        let _guard = TimerGuard::new(TaskType::WithdrawalProcessing).unwrap();

        let runtime = TestCanisterRuntime::new();
        process_pending_withdrawals(&runtime).await;

        let mut log: Log<Priority> = Log::default();
        log.push_logs(Priority::Info);
        assert!(
            log.entries.iter().any(|e| e
                .message
                .contains("failed to obtain WithdrawalProcessing guard, exiting")),
            "Expected info about failing to obtain guard, got: {:?}",
            log.entries
        );
    }

    #[tokio::test]
    async fn should_acquire_and_release_guard() {
        init_state();

        let runtime = TestCanisterRuntime::new();
        process_pending_withdrawals(&runtime).await;

        // Guard should be released, so we can acquire it again
        let _guard = TimerGuard::new(TaskType::WithdrawalProcessing).unwrap();
    }

    async fn submit_withdrawals(runtime: &TestCanisterRuntime, count: u8) {
        for i in 1..count + 1 {
            let _ = withdraw(
                runtime,
                Principal::from_slice(&[1, i]).into(),
                MINIMUM_WITHDRAWAL_AMOUNT,
                VALID_ADDRESS.to_string(),
            )
            .await
            .unwrap();
        }
    }

    #[tokio::test]
    async fn should_process_when_pending_withdrawals_exist() {
        init_state();
        init_schnorr_master_key();

        let tx_signature = signature(0x42);
        let slot = 1;

        let runtime = TestCanisterRuntime::new()
            // ledger burn response for withdraw
            .add_stub_response(Ok::<Nat, TransferFromError>(Nat::from(1u64)))
            // get_recent_slot_and_blockhash calls
            .add_stub_response(GetSlotResult::Consistent(Ok(slot)))
            .add_stub_response(GetBlockResult::Consistent(Ok(confirmed_block())))
            // schnorr signing response
            .add_signature(tx_signature.into())
            // sendTransaction response
            .add_stub_response(SendTransactionResult::Consistent(Ok(tx_signature.into())))
            .with_increasing_time();

        submit_withdrawals(&runtime, 1).await;

        process_pending_withdrawals(&runtime).await;

        EventsAssert::from_recorded()
            .expect_event(|e| {
                assert_matches!(e, EventType::AcceptedWithdrawalRequest(req) => {
                    assert_eq!(req.amount_to_burn, MINIMUM_WITHDRAWAL_AMOUNT);
                    assert_eq!(req.withdrawal_amount, MINIMUM_WITHDRAWAL_AMOUNT - WITHDRAWAL_FEE);
                });
            })
            .expect_event(|e| {
                assert_matches!(e, EventType::SubmittedTransaction {
                    signature,
                    purpose: TransactionPurpose::WithdrawSol { burn_indices },
                    ..
                } => {
                    assert_eq!(signature, tx_signature);
                    assert_eq!(*burn_indices, vec![LedgerBurnIndex::from(1u64)]);
                });
            })
            .assert_no_more_events();

        assert_matches!(withdrawal_status(1), WithdrawalStatus::TxSent(_));
    }

    #[tokio::test]
    async fn should_log_error_when_blockhash_fetch_fails() {
        init_state();

        let runtime = TestCanisterRuntime::new()
            // ledger burn response for withdraw
            .add_stub_response(Ok::<Nat, TransferFromError>(Nat::from(1u64)))
            // get_recent_block retries getSlot 3 times before giving up
            .add_stub_response(GetSlotResult::Consistent(Err(RpcError::ValidationError(
                "slot unavailable".to_string(),
            ))))
            .add_stub_response(GetSlotResult::Consistent(Err(RpcError::ValidationError(
                "slot unavailable".to_string(),
            ))))
            .add_stub_response(GetSlotResult::Consistent(Err(RpcError::ValidationError(
                "slot unavailable".to_string(),
            ))))
            .with_increasing_time();

        submit_withdrawals(&runtime, 1).await;

        process_pending_withdrawals(&runtime).await;

        // No withdrawal transaction event should be recorded
        EventsAssert::from_recorded()
            .expect_event(|e| {
                assert_matches!(e, EventType::AcceptedWithdrawalRequest(_));
            })
            .assert_no_more_events();

        let mut log: Log<Priority> = Log::default();
        log.push_logs(Priority::Info);
        assert!(
            log.entries
                .iter()
                .any(|e| e.message.contains("Failed to fetch recent blockhash")),
            "Expected info log about blockhash failure, got: {:?}",
            log.entries
        );

        assert_eq!(withdrawal_status(1), WithdrawalStatus::Pending);
    }

    #[tokio::test]
    async fn should_not_process_batch_on_sig_error() {
        init_state();
        init_schnorr_master_key();

        let slot = 1;

        let runtime = TestCanisterRuntime::new()
            // responses for burn blocks
            .add_stub_response(Ok::<Nat, TransferFromError>(Nat::from(1u64)))
            .add_stub_response(Ok::<Nat, TransferFromError>(Nat::from(2u64)))
            // get_recent_slot_and_blockhash calls
            .add_stub_response(GetSlotResult::Consistent(Ok(slot)))
            .add_stub_response(GetBlockResult::Consistent(Ok(confirmed_block())))
            // signing fails for the batch
            .add_schnorr_signing_error(SignCallError::CallFailed(
                CallRejected::with_rejection(4, "signing service unavailable".to_string()).into(),
            ))
            .with_increasing_time();

        submit_withdrawals(&runtime, 2).await;

        process_pending_withdrawals(&runtime).await;

        EventsAssert::from_recorded()
            .expect_event(|e| {
                assert_matches!(e, EventType::AcceptedWithdrawalRequest(req) => {
                    assert_eq!(req.amount_to_burn, MINIMUM_WITHDRAWAL_AMOUNT);
                    assert_eq!(req.withdrawal_amount, MINIMUM_WITHDRAWAL_AMOUNT - WITHDRAWAL_FEE);
                    assert_eq!(req.account, Principal::from_slice(&[1, 1]).into());
                });
            })
            .expect_event(|e| {
                assert_matches!(e, EventType::AcceptedWithdrawalRequest(req) => {
                    assert_eq!(req.amount_to_burn, MINIMUM_WITHDRAWAL_AMOUNT);
                    assert_eq!(req.withdrawal_amount, MINIMUM_WITHDRAWAL_AMOUNT - WITHDRAWAL_FEE);
                    assert_eq!(req.account, Principal::from_slice(&[1, 2]).into());
                });
            })
            .assert_no_more_events();

        // An error should be logged for the whole batch
        let mut log: Log<Priority> = Log::default();
        log.push_logs(Priority::Error);
        assert!(
            log.entries.iter().any(|e| e
                .message
                .contains("Failed to create batch withdrawal transaction for burn indices")),
            "Expected error log about batch sig failure, got: {:?}",
            log.entries
        );

        // Both withdrawals remain pending since they were in the same batch
        assert_matches!(withdrawal_status(1), WithdrawalStatus::Pending);
        assert_matches!(withdrawal_status(2), WithdrawalStatus::Pending);
    }

    #[tokio::test]
    async fn should_batch_withdrawals_into_transactions() {
        init_state();
        init_schnorr_master_key();

        let request_count = MAX_WITHDRAWALS_PER_TX as u64 + 1;
        let slot = 1;

        let mut runtime = TestCanisterRuntime::new().with_increasing_time();
        // withdraw ledger burn responses
        for i in 0..request_count {
            runtime = runtime.add_stub_response(Ok::<Nat, TransferFromError>(Nat::from(i)));
        }
        // get_recent_slot_and_blockhash (one round: getSlot + getBlock)
        runtime = runtime
            .add_stub_response(GetSlotResult::Consistent(Ok(slot)))
            .add_stub_response(GetBlockResult::Consistent(Ok(confirmed_block())));
        // one signature per batch + sendTransaction: batch 1 (MAX_WITHDRAWALS_PER_TX) + batch 2 (1)
        runtime = runtime
            .add_signature(signature(1).into())
            .add_stub_response(SendTransactionResult::Consistent(Ok(signature(1).into())))
            .add_signature(signature(2).into())
            .add_stub_response(SendTransactionResult::Consistent(Ok(signature(2).into())));

        submit_withdrawals(&runtime, request_count as u8).await;

        process_pending_withdrawals(&runtime).await;

        // All withdrawals should be processed in a single invocation
        // (2 batches in 1 round, both within MAX_CONCURRENT_RPC_CALLS)
        for i in 0..request_count {
            assert_matches!(withdrawal_status(i), WithdrawalStatus::TxSent(_));
        }

        // Verify events: request_count AcceptedWithdrawalRequest events,
        // then 2 SubmittedTransaction events (one per batch).
        let mut events = EventsAssert::from_recorded();
        for _ in 0..request_count {
            events = events.expect_event(|e| {
                assert_matches!(e, EventType::AcceptedWithdrawalRequest(_));
            });
        }
        events
            .expect_event(|e| {
                assert_matches!(e, EventType::SubmittedTransaction {
                    purpose: TransactionPurpose::WithdrawSol { burn_indices },
                    ..
                } => {
                    assert_eq!(burn_indices.len(), MAX_WITHDRAWALS_PER_TX);
                });
            })
            .expect_event(|e| {
                assert_matches!(e, EventType::SubmittedTransaction {
                    purpose: TransactionPurpose::WithdrawSol { burn_indices },
                    ..
                } => {
                    assert_eq!(burn_indices.len(), 1);
                });
            })
            .assert_no_more_events();
    }

    #[tokio::test]
    async fn should_process_multiple_rounds_per_invocation() {
        init_state();
        init_schnorr_master_key();

        // Create more withdrawals than fit in one round
        // (MAX_WITHDRAWALS_PER_TX * MAX_CONCURRENT_RPC_CALLS) but fewer than
        // the per-invocation limit (rounds * concurrent * per_tx).
        // This requires 2 rounds within a single invocation.
        let max_per_round = (MAX_WITHDRAWALS_PER_TX * MAX_CONCURRENT_RPC_CALLS) as u64;
        let request_count = max_per_round + 1;
        let slot = 1;

        let mut runtime = TestCanisterRuntime::new().with_increasing_time();
        for i in 0..request_count {
            runtime = runtime.add_stub_response(Ok::<Nat, TransferFromError>(Nat::from(i)));
        }

        // Round 1: get_recent_slot_and_blockhash, then signatures + sendTransaction
        runtime = runtime
            .add_stub_response(GetSlotResult::Consistent(Ok(slot)))
            .add_stub_response(GetBlockResult::Consistent(Ok(confirmed_block())));
        for i in 0..MAX_CONCURRENT_RPC_CALLS {
            runtime = runtime
                .add_signature(signature(i as u8 + 1).into())
                .add_stub_response(SendTransactionResult::Consistent(Ok(signature(
                    i as u8 + 1,
                )
                .into())));
        }

        // Round 2: fresh get_recent_slot_and_blockhash, then 1 signature + sendTransaction
        runtime = runtime
            .add_stub_response(GetSlotResult::Consistent(Ok(slot)))
            .add_stub_response(GetBlockResult::Consistent(Ok(confirmed_block())))
            .add_signature(signature(MAX_CONCURRENT_RPC_CALLS as u8 + 1).into())
            .add_stub_response(SendTransactionResult::Consistent(Ok(signature(
                MAX_CONCURRENT_RPC_CALLS as u8 + 1,
            )
            .into())));

        submit_withdrawals(&runtime, request_count as u8).await;

        // All withdrawals should be processed in a single invocation (2 rounds)
        process_pending_withdrawals(&runtime).await;

        for i in 0..request_count {
            assert_matches!(
                withdrawal_status(i),
                WithdrawalStatus::TxSent(_),
                "withdrawal {i} should be TxSent"
            );
        }
    }

    #[tokio::test]
    async fn should_respect_max_withdrawal_rounds() {
        init_state();
        init_schnorr_master_key();

        let slot = 1;
        let max_per_round = MAX_WITHDRAWALS_PER_TX * MAX_CONCURRENT_RPC_CALLS;
        // Create enough requests to fill MAX_WITHDRAWAL_ROUNDS + 1 rounds.
        let request_count = max_per_round * MAX_WITHDRAWAL_ROUNDS + 1;

        // Insert pending withdrawal requests directly into state.
        for i in 0..request_count {
            events::accept_withdrawal(account(i as u8), i as u64, MINIMUM_WITHDRAWAL_AMOUNT);
        }

        // Set up RPC responses for MAX_WITHDRAWAL_ROUNDS rounds.
        let mut runtime = TestCanisterRuntime::new().with_increasing_time();
        let mut sig_counter: u8 = 0;
        for _round in 0..MAX_WITHDRAWAL_ROUNDS {
            runtime = runtime
                .add_stub_response(GetSlotResult::Consistent(Ok(slot)))
                .add_stub_response(GetBlockResult::Consistent(Ok(confirmed_block())));
            for _ in 0..MAX_CONCURRENT_RPC_CALLS {
                sig_counter = sig_counter.wrapping_add(1);
                runtime = runtime
                    .add_signature(signature(sig_counter).into())
                    .add_stub_response(SendTransactionResult::Consistent(Ok(signature(
                        sig_counter,
                    )
                    .into())));
            }
        }

        process_pending_withdrawals(&runtime).await;

        let processed = max_per_round * MAX_WITHDRAWAL_ROUNDS;
        // All requests within MAX_WITHDRAWAL_ROUNDS rounds should be processed
        for i in 0..processed {
            assert_matches!(
                withdrawal_status(i as u64),
                WithdrawalStatus::TxSent(_),
                "withdrawal {i} should be TxSent"
            );
        }
        // The extra request beyond MAX_WITHDRAWAL_ROUNDS rounds should remain pending
        assert_matches!(
            withdrawal_status(processed as u64),
            WithdrawalStatus::Pending,
            "withdrawal beyond max rounds should still be Pending"
        );
    }
}

mod withdrawal_finalization_tests {
    use super::*;

    fn setup_sent_withdrawal(burn_block_index: u64) -> Signature {
        let tx_signature = signature(burn_block_index as u8 + 1);
        events::accept_withdrawal(MINTER_ACCOUNT, burn_block_index, MINIMUM_WITHDRAWAL_AMOUNT);
        events::submit_withdrawal(tx_signature, MINTER_ACCOUNT, 1, vec![burn_block_index]);
        tx_signature
    }

    #[test]
    fn should_report_tx_finalized_after_succeeded_transaction() {
        init_state();
        let tx_signature = setup_sent_withdrawal(1);

        assert_matches!(withdrawal_status(1), WithdrawalStatus::TxSent(_));

        events::succeed_transaction(tx_signature);

        assert_matches!(
            withdrawal_status(1),
            WithdrawalStatus::TxFinalized(TxFinalizedStatus::Success { transaction_hash, .. })
                if transaction_hash == tx_signature.to_string()
        );
    }

    #[test]
    fn should_report_tx_failed_after_failed_transaction() {
        init_state();
        let tx_signature = setup_sent_withdrawal(1);

        assert_matches!(withdrawal_status(1), WithdrawalStatus::TxSent(_));

        events::fail_transaction(tx_signature);

        assert_matches!(
            withdrawal_status(1),
            WithdrawalStatus::TxFinalized(TxFinalizedStatus::Failure { transaction_hash })
                if transaction_hash == tx_signature.to_string()
        );
    }
}
