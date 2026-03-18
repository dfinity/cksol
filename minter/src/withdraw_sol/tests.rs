use crate::{
    guard::{TimerGuard, withdraw_sol_guard},
    state::TaskType,
    test_fixtures::{
        MINTER_ACCOUNT, WITHDRAWAL_FEE, init_schnorr_master_key, init_state,
        runtime::TestCanisterRuntime,
    },
    withdraw_sol::{process_pending_withdrawals, process_pending_withdrawals_with_signer, withdraw_sol},
};
use assert_matches::assert_matches;
use candid::{Nat, Principal};
use cksol_types::{WithdrawSolError, WithdrawSolOk};
use ic_canister_runtime::IcError;
use icrc_ledger_types::{icrc1::account::Account, icrc2::transfer_from::TransferFromError};

const VALID_ADDRESS: &str = "E4MpwNnMWs2XtW5gVrxZvyS7fMq31QD5HvbxmwP45Tz3";

fn test_caller() -> Principal {
    Principal::from_slice(&[1_u8; 20])
}

#[tokio::test]
async fn should_return_error_if_calling_ledger_fails() {
    init_state();

    let runtime = TestCanisterRuntime::new().add_stub_error(IcError::CallPerformFailed);

    let result = withdraw_sol(
        runtime,
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
        runtime,
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
        runtime,
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
        runtime,
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
        runtime,
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
        runtime,
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
        runtime,
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
        runtime,
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
        runtime,
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
    use crate::sol_transfer::SchnorrSigner;
    use crate::address::DerivationPath;
    use ic_cdk::management_canister::SignCallError;
    use std::cell::RefCell;
    use std::collections::VecDeque;

    struct MockSchnorrSigner {
        responses: RefCell<VecDeque<Result<Vec<u8>, SignCallError>>>,
    }

    impl MockSchnorrSigner {
        fn with_signatures(signatures: Vec<[u8; 64]>) -> Self {
            Self {
                responses: RefCell::new(
                    signatures.into_iter().map(|sig| Ok(sig.to_vec())).collect(),
                ),
            }
        }
    }

    impl SchnorrSigner for MockSchnorrSigner {
        async fn sign(
            &self,
            _message: Vec<u8>,
            _derivation_path: DerivationPath,
        ) -> Result<Vec<u8>, SignCallError> {
            self.responses
                .borrow_mut()
                .pop_front()
                .expect("MockSchnorrSigner: no more stub responses")
        }
    }

    #[tokio::test]
    async fn should_do_nothing_if_no_pending_withdrawals() {
        init_state();

        let runtime = TestCanisterRuntime::new();
        process_pending_withdrawals(runtime).await;
    }

    #[tokio::test]
    async fn should_skip_if_already_processing() {
        init_state();

        let _guard = TimerGuard::new(TaskType::WithdrawalProcessing).unwrap();

        let runtime = TestCanisterRuntime::new();
        process_pending_withdrawals(runtime).await;
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
    async fn should_process_when_pending_withdrawals_exist() {
        init_state();
        init_schnorr_master_key();

        let minter_self = Principal::from_slice(&[0, 1, 2, 3, 4]);

        // Create a pending withdrawal by accepting one
        let runtime = TestCanisterRuntime::new()
            .add_stub_response(Ok::<Nat, TransferFromError>(Nat::from(1u64)))
            .with_canister_self(minter_self)
            .with_increasing_time();

        let _ = withdraw_sol(
            runtime,
            MINTER_ACCOUNT,
            test_caller(),
            None,
            WITHDRAWAL_FEE + 1,
            VALID_ADDRESS.to_string(),
        )
        .await
        .unwrap();

        type SendSlotResult = sol_rpc_types::MultiRpcResult<sol_rpc_types::Slot>;
        type SendBlockResult = sol_rpc_types::MultiRpcResult<sol_rpc_types::ConfirmedBlock>;

        let runtime = TestCanisterRuntime::new()
            // estimate_recent_blockhash: getSlot + getBlock
            .add_stub_response(SendSlotResult::Consistent(Ok(1)))
            .add_stub_response(SendBlockResult::Consistent(Ok(
                sol_rpc_types::ConfirmedBlock {
                    previous_blockhash: Default::default(),
                    blockhash: solana_hash::Hash::new_from_array([0x42; 32]).into(),
                    parent_slot: 0,
                    block_time: None,
                    block_height: None,
                    signatures: None,
                    rewards: None,
                    num_reward_partitions: None,
                    transactions: None,
                },
            )))
            .with_canister_self(minter_self)
            .with_increasing_time();

        let signer = MockSchnorrSigner::with_signatures(vec![[0x42; 64]]);
        process_pending_withdrawals_with_signer(runtime, &signer).await;
    }
}
