use crate::numeric::LedgerBurnIndex;
use crate::sol_transfer::MAX_WITHDRAWALS_PER_TX;
use crate::test_fixtures::EventsAssert;
use crate::{
    guard::{TimerGuard, withdraw_sol_guard},
    state::TaskType,
    state::event::{EventType, TransactionPurpose},
    test_fixtures::{
        MINTER_ACCOUNT, WITHDRAWAL_FEE, init_schnorr_master_key, init_state,
        runtime::TestCanisterRuntime,
    },
    withdraw_sol::{
        MAX_CONCURRENT_WITHDRAWAL_TXS, process_pending_withdrawals, withdraw_sol,
        withdraw_sol_status,
    },
};
use assert_matches::assert_matches;
use candid::{Nat, Principal};
use canlog::Log;
use cksol_types::WithdrawSolStatus;
use cksol_types::{WithdrawSolError, WithdrawSolOk};
use cksol_types_internal::log::Priority;
use ic_canister_runtime::IcError;
use ic_cdk::call::CallRejected;
use ic_cdk::management_canister::SignCallError;
use icrc_ledger_types::{icrc1::account::Account, icrc2::transfer_from::TransferFromError};
use sol_rpc_types::{ConfirmedBlock, MultiRpcResult, RpcError, Slot};
use solana_signature::Signature;

const VALID_ADDRESS: &str = "E4MpwNnMWs2XtW5gVrxZvyS7fMq31QD5HvbxmwP45Tz3";

fn test_caller() -> Principal {
    Principal::from_slice(&[1_u8; 20])
}

#[tokio::test]
async fn should_return_error_if_calling_ledger_fails() {
    init_state();

    let runtime = TestCanisterRuntime::new().add_stub_error(IcError::CallPerformFailed);

    let result = withdraw_sol(
        &runtime,
        MINTER_ACCOUNT,
        test_caller(),
        None,
        1,
        VALID_ADDRESS.to_string(),
    )
    .await;

    assert_matches!(
        result,
        Err(WithdrawSolError::TemporarilyUnavailable(e)) => assert!(e.contains("Failed to burn tokens"))
    );
}

#[tokio::test]
async fn should_return_error_if_ledger_unavailable() {
    init_state();

    let runtime = TestCanisterRuntime::new().add_stub_response(Err::<Nat, TransferFromError>(
        TransferFromError::TemporarilyUnavailable,
    ));

    let result = withdraw_sol(
        &runtime,
        MINTER_ACCOUNT,
        test_caller(),
        None,
        1,
        VALID_ADDRESS.to_string(),
    )
    .await;

    assert_eq!(
        result,
        Err(WithdrawSolError::TemporarilyUnavailable(
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

    let result = withdraw_sol(
        &runtime,
        MINTER_ACCOUNT,
        test_caller(),
        None,
        1,
        VALID_ADDRESS.to_string(),
    )
    .await;

    assert_eq!(
        result,
        Err(WithdrawSolError::InsufficientAllowance { allowance: 123u64 })
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

    let result = withdraw_sol(
        &runtime,
        MINTER_ACCOUNT,
        test_caller(),
        None,
        1,
        VALID_ADDRESS.to_string(),
    )
    .await;

    assert_eq!(
        result,
        Err(WithdrawSolError::InsufficientFunds { balance: 123u64 })
    );
}

#[tokio::test]
async fn should_return_generic_error() {
    init_state();

    let runtime = TestCanisterRuntime::new().add_stub_response(Err::<Nat, TransferFromError>(
        TransferFromError::GenericError {
            error_code: Nat::from(123u64),
            message: "msg".to_string(),
        },
    ));

    let result = withdraw_sol(
        &runtime,
        MINTER_ACCOUNT,
        test_caller(),
        None,
        1,
        VALID_ADDRESS.to_string(),
    )
    .await;

    assert_eq!(
        result,
        Err(WithdrawSolError::GenericError {
            error_message: "msg".to_string(),
            error_code: 123u64
        })
    );
}

#[tokio::test]
async fn should_return_ok_if_burn_succeeds() {
    init_state();

    let runtime = TestCanisterRuntime::new()
        .add_stub_response(Ok::<Nat, TransferFromError>(Nat::from(123u64)))
        .with_increasing_time();

    let result = withdraw_sol(
        &runtime,
        MINTER_ACCOUNT,
        test_caller(),
        None,
        1,
        VALID_ADDRESS.to_string(),
    )
    .await;

    assert_eq!(
        result,
        Ok(WithdrawSolOk {
            block_index: 123u64
        })
    );
}

#[tokio::test]
async fn should_return_error_if_address_malformed() {
    init_state();

    let runtime = TestCanisterRuntime::new();

    let result = withdraw_sol(
        &runtime,
        MINTER_ACCOUNT,
        test_caller(),
        None,
        1,
        "not-a-valid-address".to_string(),
    )
    .await;

    assert_matches!(result, Err(WithdrawSolError::MalformedAddress(_)));
}

#[tokio::test]
#[should_panic(expected = "the owner must be non-anonymous")]
async fn should_panic_if_caller_is_anonymous() {
    init_state();

    let runtime = TestCanisterRuntime::new();

    let _ = withdraw_sol(
        &runtime,
        MINTER_ACCOUNT,
        Principal::anonymous(),
        None,
        1,
        VALID_ADDRESS.to_string(),
    )
    .await;
}

#[tokio::test]
async fn should_return_error_if_already_processing() {
    init_state();

    let caller = test_caller();
    let from = Account {
        owner: caller,
        subaccount: None,
    };
    let _guard = withdraw_sol_guard(from).unwrap();

    let runtime = TestCanisterRuntime::new();

    let result = withdraw_sol(
        &runtime,
        MINTER_ACCOUNT,
        caller,
        None,
        1,
        VALID_ADDRESS.to_string(),
    )
    .await;

    assert_eq!(result, Err(WithdrawSolError::AlreadyProcessing));
}

mod process_pending_withdrawals_tests {
    use super::*;

    type SendSlotResult = MultiRpcResult<Slot>;
    type SendBlockResult = MultiRpcResult<ConfirmedBlock>;

    fn signature(index: u8) -> Signature {
        Signature::from([index; 64])
    }

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

    async fn withdraw(runtime: &TestCanisterRuntime, count: u8) {
        for i in 1..count + 1 {
            let _ = withdraw_sol(
                runtime,
                MINTER_ACCOUNT,
                Principal::from_slice(&[1, i]),
                None,
                WITHDRAWAL_FEE + 1,
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

        let fake_sig = [0x42; 64];
        let slot = 1;

        let runtime = TestCanisterRuntime::new()
            // ledger burn response for withdraw_sol
            .add_stub_response(Ok::<Nat, TransferFromError>(Nat::from(1u64)))
            // responses for recent block hash
            .add_stub_response(SendSlotResult::Consistent(Ok(slot)))
            .add_stub_response(SendBlockResult::Consistent(Ok(get_confirmed_block())))
            .add_stub_response(SendSlotResult::Consistent(Ok(slot)))
            // schnorr signing response
            .add_signature(fake_sig)
            .with_increasing_time();

        withdraw(&runtime, 1).await;

        process_pending_withdrawals(&runtime).await;

        EventsAssert::from_recorded()
            .expect_event(|e| {
                assert_matches!(e, EventType::AcceptedWithdrawSolRequest(req) => {
                    assert_eq!(req.withdrawal_amount, WITHDRAWAL_FEE + 1);
                    assert_eq!(req.withdrawal_fee, WITHDRAWAL_FEE);
                });
            })
            .expect_event(|e| {
                assert_matches!(e, EventType::SubmittedTransaction {
                    signature,
                    purpose: TransactionPurpose::WithdrawSol { burn_indices },
                    ..
                } => {
                    assert_eq!(signature, Signature::from(fake_sig));
                    assert_eq!(*burn_indices, vec![LedgerBurnIndex::from(1u64)]);
                });
            })
            .assert_no_more_events();

        assert_matches!(withdraw_sol_status(1), WithdrawSolStatus::TxSent(_));
    }

    #[tokio::test]
    async fn should_log_error_when_blockhash_fetch_fails() {
        init_state();

        let runtime = TestCanisterRuntime::new()
            // ledger burn response for withdraw_sol
            .add_stub_response(Ok::<Nat, TransferFromError>(Nat::from(1u64)))
            // estimate_recent_blockhash retries getSlot 3 times before giving up
            .add_stub_response(SendSlotResult::Consistent(Err(RpcError::ValidationError(
                "slot unavailable".to_string(),
            ))))
            .add_stub_response(SendSlotResult::Consistent(Err(RpcError::ValidationError(
                "slot unavailable".to_string(),
            ))))
            .add_stub_response(SendSlotResult::Consistent(Err(RpcError::ValidationError(
                "slot unavailable".to_string(),
            ))))
            .with_increasing_time();

        withdraw(&runtime, 1).await;

        process_pending_withdrawals(&runtime).await;

        // No withdrawal transaction event should be recorded
        EventsAssert::from_recorded()
            .expect_event(|e| {
                assert_matches!(e, EventType::AcceptedWithdrawSolRequest(_));
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

        assert_eq!(withdraw_sol_status(1), WithdrawSolStatus::Pending);
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
            // responses for recent block hash
            .add_stub_response(SendSlotResult::Consistent(Ok(slot)))
            .add_stub_response(SendBlockResult::Consistent(Ok(get_confirmed_block())))
            .add_stub_response(SendSlotResult::Consistent(Ok(slot)))
            // signing fails for the batch
            .add_schnorr_signing_error(SignCallError::CallFailed(
                CallRejected::with_rejection(4, "signing service unavailable".to_string()).into(),
            ))
            .with_increasing_time();

        withdraw(&runtime, 2).await;

        process_pending_withdrawals(&runtime).await;

        EventsAssert::from_recorded()
            .expect_event(|e| {
                assert_matches!(e, EventType::AcceptedWithdrawSolRequest(req) => {
                    assert_eq!(req.withdrawal_amount, WITHDRAWAL_FEE + 1);
                    assert_eq!(req.withdrawal_fee, WITHDRAWAL_FEE);
                    assert_eq!(req.account, Principal::from_slice(&[1, 1]).into());
                });
            })
            .expect_event(|e| {
                assert_matches!(e, EventType::AcceptedWithdrawSolRequest(req) => {
                    assert_eq!(req.withdrawal_amount, WITHDRAWAL_FEE + 1);
                    assert_eq!(req.withdrawal_fee, WITHDRAWAL_FEE);
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
        assert_matches!(withdraw_sol_status(1), WithdrawSolStatus::Pending);
        assert_matches!(withdraw_sol_status(2), WithdrawSolStatus::Pending);
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
        // responses for recent block hash (one round)
        runtime = runtime
            .add_stub_response(SendSlotResult::Consistent(Ok(slot)))
            .add_stub_response(SendBlockResult::Consistent(Ok(get_confirmed_block())))
            .add_stub_response(SendSlotResult::Consistent(Ok(slot)));
        // one signature per batch: batch 1 (MAX_WITHDRAWALS_PER_TX) + batch 2 (1)
        runtime = runtime
            .add_signature(signature(1).into())
            .add_signature(signature(2).into());

        withdraw(&runtime, request_count as u8).await;

        process_pending_withdrawals(&runtime).await;

        // All withdrawals should be processed in a single invocation
        // (2 batches in 1 round, both within MAX_CONCURRENT_WITHDRAWAL_TXS)
        for i in 0..request_count {
            assert_matches!(withdraw_sol_status(i), WithdrawSolStatus::TxSent(_));
        }

        // Verify events: request_count AcceptedWithdrawSolRequest events,
        // then 2 SubmittedTransaction events (one per batch).
        let mut events = EventsAssert::from_recorded();
        for _ in 0..request_count {
            events = events.expect_event(|e| {
                assert_matches!(e, EventType::AcceptedWithdrawSolRequest(_));
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
    async fn should_respect_max_concurrent_transactions() {
        init_state();
        init_schnorr_master_key();

        // Create more withdrawals than one invocation can handle.
        // One call processes at most MAX_WITHDRAWALS_PER_TX * MAX_CONCURRENT_WITHDRAWAL_TXS.
        let max_per_invocation =
            (MAX_WITHDRAWALS_PER_TX * MAX_CONCURRENT_WITHDRAWAL_TXS) as u64;
        let request_count = max_per_invocation + 1;
        let slot = 1;

        let mut runtime = TestCanisterRuntime::new().with_increasing_time();
        // withdraw ledger burn responses
        for i in 0..request_count {
            runtime = runtime.add_stub_response(Ok::<Nat, TransferFromError>(Nat::from(i)));
        }

        // First invocation: blockhash + slot, then MAX_CONCURRENT_WITHDRAWAL_TXS signatures
        runtime = runtime
            .add_stub_response(SendSlotResult::Consistent(Ok(slot)))
            .add_stub_response(SendBlockResult::Consistent(Ok(get_confirmed_block())))
            .add_stub_response(SendSlotResult::Consistent(Ok(slot)));
        for i in 0..MAX_CONCURRENT_WITHDRAWAL_TXS {
            runtime = runtime.add_signature(signature(i as u8 + 1).into());
        }

        // Second invocation: blockhash + slot, then 1 signature for the remaining batch
        runtime = runtime
            .add_stub_response(SendSlotResult::Consistent(Ok(slot)))
            .add_stub_response(SendBlockResult::Consistent(Ok(get_confirmed_block())))
            .add_stub_response(SendSlotResult::Consistent(Ok(slot)))
            .add_signature(signature(MAX_CONCURRENT_WITHDRAWAL_TXS as u8 + 1).into());

        withdraw(&runtime, request_count as u8).await;

        // First invocation processes up to the limit
        process_pending_withdrawals(&runtime).await;

        for i in 0..max_per_invocation {
            assert_matches!(
                withdraw_sol_status(i),
                WithdrawSolStatus::TxSent(_),
                "withdrawal {i} should be TxSent after first invocation"
            );
        }
        // The last withdrawal should still be pending
        assert_matches!(
            withdraw_sol_status(max_per_invocation),
            WithdrawSolStatus::Pending
        );

        // Second invocation picks up the remaining withdrawal
        process_pending_withdrawals(&runtime).await;

        for i in 0..request_count {
            assert_matches!(
                withdraw_sol_status(i),
                WithdrawSolStatus::TxSent(_),
                "withdrawal {i} should be TxSent after second invocation"
            );
        }
    }

    fn get_confirmed_block() -> ConfirmedBlock {
        ConfirmedBlock {
            previous_blockhash: Default::default(),
            blockhash: solana_hash::Hash::new_from_array([0x42; 32]).into(),
            parent_slot: 0,
            block_time: None,
            block_height: None,
            signatures: None,
            rewards: None,
            num_reward_partitions: None,
            transactions: None,
        }
    }
}
