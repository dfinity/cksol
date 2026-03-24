use crate::numeric::LedgerBurnIndex;
use crate::test_fixtures::EventsAssert;
use crate::withdraw_sol::MAX_WITHDRAWALS_PER_BATCH;
use crate::{
    guard::{TimerGuard, withdraw_sol_guard},
    state::TaskType,
    test_fixtures::{
        MINTER_ACCOUNT, WITHDRAWAL_FEE, init_schnorr_master_key, init_state,
        runtime::TestCanisterRuntime,
    },
    withdraw_sol::{process_pending_withdrawals, withdraw_sol},
};
use crate::{state::event::EventType, withdraw_sol::withdraw_sol_status};
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
            log.entries
                .iter()
                .any(|e| e.message.contains("failed to obtain guard, exiting")),
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

        let runtime = TestCanisterRuntime::new()
            // ledger burn response for withdraw_sol
            .add_stub_response(Ok::<Nat, TransferFromError>(Nat::from(1u64)))
            // responses for recent block hash
            .add_stub_response(SendSlotResult::Consistent(Ok(1)))
            .add_stub_response(SendBlockResult::Consistent(Ok(get_confirmed_block())))
            // schnorr signing response
            .add_schnorr_signature(fake_sig)
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
                assert_matches!(e, EventType::SentWithdrawalTransaction { signature, .. } => {
                    assert_eq!(signature, fake_sig.into());
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

        // An error should be logged
        let mut log: Log<Priority> = Log::default();
        log.push_logs(Priority::Error);
        assert!(
            log.entries
                .iter()
                .any(|e| e.message.contains("Failed to estimate recent blockhash")),
            "Expected error log about blockhash failure, got: {:?}",
            log.entries
        );

        assert_eq!(withdraw_sol_status(1), WithdrawSolStatus::Pending);
    }

    #[tokio::test]
    async fn should_not_process_pending_withdrawal_sig_error() {
        init_state();
        init_schnorr_master_key();

        let runtime = TestCanisterRuntime::new()
            // responses for burn blocks
            .add_stub_response(Ok::<Nat, TransferFromError>(Nat::from(1u64)))
            .add_stub_response(Ok::<Nat, TransferFromError>(Nat::from(2u64)))
            // responses for recent block hash
            .add_stub_response(SendSlotResult::Consistent(Ok(1)))
            .add_stub_response(SendBlockResult::Consistent(Ok(get_confirmed_block())))
            // one successful signature and one failed
            .add_schnorr_signature([0x42; 64])
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
            .expect_event(|e| {
                assert_matches!(e, EventType::SentWithdrawalTransaction { burn_block_index, .. } => {
                    assert_eq!(burn_block_index, LedgerBurnIndex::from(1u64));
                });
            })
            .assert_no_more_events();

        // An error should be logged
        let mut log: Log<Priority> = Log::default();
        log.push_logs(Priority::Error);
        assert!(
            log.entries.iter().any(|e| e
                .message
                .contains("Failed to create withdrawal transaction for burn index 2")),
            "Expected error log about sig failure, got: {:?}",
            log.entries
        );

        assert_matches!(withdraw_sol_status(1), WithdrawSolStatus::TxSent(_));
        assert_matches!(withdraw_sol_status(2), WithdrawSolStatus::Pending);
    }

    #[tokio::test]
    async fn should_process_up_to_max() {
        init_state();
        init_schnorr_master_key();

        let request_count = MAX_WITHDRAWALS_PER_BATCH as u64 + 1;

        let mut runtime = TestCanisterRuntime::new().with_increasing_time();
        // withdraw ledger burn responses
        for i in 0..request_count {
            runtime = runtime.add_stub_response(Ok::<Nat, TransferFromError>(Nat::from(i)));
        }
        // responses for recent block hash
        runtime = runtime
            .add_stub_response(SendSlotResult::Consistent(Ok(1)))
            .add_stub_response(SendBlockResult::Consistent(Ok(get_confirmed_block())));
        // signatures for the first batch of withdrawals
        for _ in 0..MAX_WITHDRAWALS_PER_BATCH {
            runtime = runtime.add_schnorr_signature([0x42; 64]);
        }
        // responses for recent block hash and signature for the second batch
        runtime = runtime
            .add_stub_response(SendSlotResult::Consistent(Ok(1)))
            .add_stub_response(SendBlockResult::Consistent(Ok(get_confirmed_block())))
            .add_schnorr_signature([0x42; 64]);

        withdraw(&runtime, request_count as u8).await;

        process_pending_withdrawals(&runtime).await;

        for i in 0..MAX_WITHDRAWALS_PER_BATCH {
            assert_matches!(withdraw_sol_status(i as u64), WithdrawSolStatus::TxSent(_));
        }
        // last withdrawal was not yet processed
        assert_matches!(
            withdraw_sol_status(MAX_WITHDRAWALS_PER_BATCH as u64),
            WithdrawSolStatus::Pending
        );

        process_pending_withdrawals(&runtime).await;

        // all withdrawals are now processed
        for i in 0..request_count {
            assert_matches!(withdraw_sol_status(i), WithdrawSolStatus::TxSent(_));
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
