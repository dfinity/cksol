use crate::{
    constants::MAX_CONCURRENT_RPC_CALLS,
    guard::{TimerGuard, withdrawal_guard},
    sol_transfer::MAX_WITHDRAWALS_PER_TX,
    state::{TaskType, read_state},
    test_fixtures::{
        EventsAssert, MINIMUM_WITHDRAWAL_AMOUNT, MINTER_ACCOUNT, WITHDRAWAL_FEE, account,
        confirmed_block, events, init_balance, init_balance_to, init_schnorr_master_key,
        init_state, runtime::TestCanisterRuntime, signature,
    },
    withdraw::{process_pending_withdrawals, withdraw, withdrawal_status},
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
    use super::*;

    type GetSlotResult = MultiRpcResult<Slot>;
    type GetBlockResult = MultiRpcResult<sol_rpc_types::ConfirmedBlock>;
    type SendTransactionResult = MultiRpcResult<sol_rpc_types::Signature>;

    #[tokio::test]
    async fn should_do_nothing_if_no_pending_withdrawals() {
        init_state();

        // We return early, therefore no RPC calls are made
        let runtime = TestCanisterRuntime::new();
        process_pending_withdrawals(runtime).await;

        EventsAssert::assert_no_events_recorded();
    }

    #[tokio::test]
    async fn should_skip_if_already_processing() {
        init_state();

        let _guard = TimerGuard::new(TaskType::WithdrawalProcessing).unwrap();

        // We return early, therefore no RPC calls are made
        let runtime = TestCanisterRuntime::new();
        process_pending_withdrawals(runtime).await;

        EventsAssert::assert_no_events_recorded();
    }

    #[tokio::test]
    async fn should_acquire_and_release_guard() {
        init_state();

        let runtime = TestCanisterRuntime::new();
        process_pending_withdrawals(runtime).await;

        // Guard should be released, so we can acquire it again
        let _guard = TimerGuard::new(TaskType::WithdrawalProcessing).unwrap();
    }

    #[tokio::test]
    async fn should_skip_withdrawals_when_balance_insufficient() {
        init_state();
        // No init_balance call, so minter balance is 0
        init_schnorr_master_key();

        events::accept_withdrawal(account(1), 0, MINIMUM_WITHDRAWAL_AMOUNT);

        let events_before = EventsAssert::from_recorded();

        let runtime = TestCanisterRuntime::new().with_increasing_time();
        process_pending_withdrawals(runtime).await;

        // No new events should be recorded
        let events_after = EventsAssert::from_recorded();
        assert_eq!(events_before, events_after);

        // Withdrawal should remain pending (not submitted)
        assert_eq!(withdrawal_status(0), WithdrawalStatus::Pending);
    }

    #[tokio::test]
    async fn should_process_only_affordable_withdrawals() {
        init_state();
        init_balance_to(12_500_000);
        init_schnorr_master_key();

        let tx_signature = signature(0x42);
        let slot = 1;

        // The minter balance is sufficient for the first two withdrawals
        events::accept_withdrawal(account(1), 0, 5_000_000 + WITHDRAWAL_FEE);
        events::accept_withdrawal(account(2), 1, 5_000_000 + WITHDRAWAL_FEE);
        events::accept_withdrawal(account(3), 2, 5_000_000 + WITHDRAWAL_FEE);

        let events_before = EventsAssert::from_recorded();

        let runtime = TestCanisterRuntime::new()
            .add_stub_response(GetSlotResult::Consistent(Ok(slot)))
            .add_stub_response(GetBlockResult::Consistent(Ok(confirmed_block())))
            .add_signature(tx_signature.into())
            .add_stub_response(SendTransactionResult::Consistent(Ok(tx_signature.into())))
            .with_increasing_time();

        process_pending_withdrawals(runtime).await;

        // First two withdrawals should be submitted, third should remain pending
        assert_matches!(withdrawal_status(0), WithdrawalStatus::TxSent(_));
        assert_matches!(withdrawal_status(1), WithdrawalStatus::TxSent(_));
        assert_eq!(withdrawal_status(2), WithdrawalStatus::Pending);

        // One new event (the submitted transaction batching both withdrawals)
        let events_after = EventsAssert::from_recorded();
        assert_eq!(events_after.len(), events_before.len() + 1);
    }

    #[tokio::test]
    async fn should_process_when_pending_withdrawals_exist() {
        init_state();
        init_balance();
        init_schnorr_master_key();

        let tx_signature = signature(0x42);
        let slot = 1;
        events::accept_withdrawal(account(1), 1, MINIMUM_WITHDRAWAL_AMOUNT);

        let runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .add_stub_response(GetSlotResult::Consistent(Ok(slot)))
            .add_stub_response(GetBlockResult::Consistent(Ok(confirmed_block())))
            .add_signature(tx_signature.into())
            .add_stub_response(SendTransactionResult::Consistent(Ok(tx_signature.into())));

        process_pending_withdrawals(runtime).await;

        assert_matches!(withdrawal_status(1), WithdrawalStatus::TxSent(_));
    }

    #[tokio::test]
    async fn should_log_error_when_blockhash_fetch_fails() {
        init_state();
        init_balance();

        events::accept_withdrawal(account(1), 1, MINIMUM_WITHDRAWAL_AMOUNT);

        let events_before = EventsAssert::from_recorded();

        let runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .add_stub_response(GetSlotResult::Consistent(Err(RpcError::ValidationError(
                "slot unavailable".to_string(),
            ))))
            .add_stub_response(GetSlotResult::Consistent(Err(RpcError::ValidationError(
                "slot unavailable".to_string(),
            ))))
            .add_stub_response(GetSlotResult::Consistent(Err(RpcError::ValidationError(
                "slot unavailable".to_string(),
            ))));

        process_pending_withdrawals(runtime).await;

        // No withdrawal transaction event should be recorded
        let events_after = EventsAssert::from_recorded();
        assert_eq!(events_before, events_after);

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
        init_balance();
        init_schnorr_master_key();

        let slot = 1;
        events::accept_withdrawal(account(1), 1, MINIMUM_WITHDRAWAL_AMOUNT);
        events::accept_withdrawal(account(2), 2, MINIMUM_WITHDRAWAL_AMOUNT);

        let events_before = EventsAssert::from_recorded();

        let runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .add_stub_response(GetSlotResult::Consistent(Ok(slot)))
            .add_stub_response(GetBlockResult::Consistent(Ok(confirmed_block())))
            .add_schnorr_signing_error(SignCallError::CallFailed(
                CallRejected::with_rejection(4, "signing service unavailable".to_string()).into(),
            ));

        process_pending_withdrawals(runtime).await;

        // No transaction event should be recorded (signing failed)
        let events_after = EventsAssert::from_recorded();
        assert_eq!(events_before, events_after);

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
        init_balance();
        init_schnorr_master_key();

        let request_count = MAX_WITHDRAWALS_PER_TX as u64 + 1;
        let slot = 1;

        for i in 0..request_count {
            events::accept_withdrawal(account(i as usize), i, MINIMUM_WITHDRAWAL_AMOUNT);
        }

        let runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .add_stub_response(GetSlotResult::Consistent(Ok(slot)))
            .add_stub_response(GetBlockResult::Consistent(Ok(confirmed_block())))
            .add_signature(signature(1).into())
            .add_stub_response(SendTransactionResult::Consistent(Ok(signature(1).into())))
            .add_signature(signature(2).into())
            .add_stub_response(SendTransactionResult::Consistent(Ok(signature(2).into())));

        process_pending_withdrawals(runtime).await;

        // All withdrawals should be processed in a single invocation
        // (2 batches in 1 round, both within MAX_CONCURRENT_RPC_CALLS)
        for i in 0..request_count {
            assert_matches!(withdrawal_status(i), WithdrawalStatus::TxSent(_));
        }

        // Verify that withdrawals were split into 2 batches
        read_state(|s| assert_eq!(s.submitted_transactions().len(), 2));
    }

    #[tokio::test]
    async fn should_reschedule_until_all_withdrawals_processed() {
        init_state();
        init_balance();
        init_schnorr_master_key();

        let num_requests = MAX_WITHDRAWALS_PER_TX * MAX_CONCURRENT_RPC_CALLS + 1;
        for i in 0..num_requests {
            events::accept_withdrawal(account(i), i as u64, MINIMUM_WITHDRAWAL_AMOUNT);
        }

        let slot = 1;

        // Round 1: processes MAX_CONCURRENT_RPC_CALLS batches, 1 request remains → reschedule
        let mut runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .add_stub_response(GetSlotResult::Consistent(Ok(slot)))
            .add_stub_response(GetBlockResult::Consistent(Ok(confirmed_block())));
        for i in 0..MAX_CONCURRENT_RPC_CALLS {
            runtime = runtime
                .add_signature(signature(i + 1).into())
                .add_stub_response(SendTransactionResult::Consistent(Ok(
                    signature(i + 1).into()
                )));
        }

        process_pending_withdrawals(runtime.clone()).await;

        assert!(!read_state(|s| s.pending_withdrawal_requests().is_empty()));
        assert_eq!(runtime.set_timer_call_count(), 1);

        // Round 2: processes the remaining 1 request → no reschedule
        let last_sig = signature(num_requests);
        let runtime2 = TestCanisterRuntime::new()
            .with_increasing_time()
            .add_stub_response(GetSlotResult::Consistent(Ok(slot)))
            .add_stub_response(GetBlockResult::Consistent(Ok(confirmed_block())))
            .add_signature(last_sig.into())
            .add_stub_response(SendTransactionResult::Consistent(Ok(last_sig.into())));

        process_pending_withdrawals(runtime2.clone()).await;

        assert!(read_state(|s| s.pending_withdrawal_requests().is_empty()));
        assert_eq!(runtime2.set_timer_call_count(), 0);
    }
}

mod withdrawal_finalization_tests {
    use super::*;

    fn setup_sent_withdrawal(burn_block_index: u64) -> Signature {
        let tx_signature = signature(burn_block_index as usize + 1);
        events::accept_withdrawal(MINTER_ACCOUNT, burn_block_index, MINIMUM_WITHDRAWAL_AMOUNT);
        events::submit_withdrawal(tx_signature, MINTER_ACCOUNT, 1, vec![burn_block_index]);
        tx_signature
    }

    #[test]
    fn should_report_tx_finalized_after_succeeded_transaction() {
        init_state();
        init_balance();
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
        init_balance();
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
