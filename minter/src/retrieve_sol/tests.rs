use crate::{
    guard::retrieve_sol_guard, retrieve_sol::retrieve_sol, runtime::TestCanisterRuntime,
    test_fixtures::init_state,
};
use assert_matches::assert_matches;
use candid::{Nat, Principal};
use cksol_types::{RetrieveSolError, RetrieveSolOk};
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

    let minter_account = Principal::anonymous().into();

    let result = retrieve_sol(
        runtime,
        minter_account,
        test_caller(),
        None,
        1,
        VALID_ADDRESS.to_string(),
    )
    .await;

    assert_matches!(
        result,
        Err(RetrieveSolError::TemporarilyUnavailable(e)) => assert!(e.contains("Failed to burn tokens"))
    );
}

#[tokio::test]
async fn should_return_error_if_ledger_unavailable() {
    init_state();

    let runtime = TestCanisterRuntime::new().add_stub_response(Err::<Nat, TransferFromError>(
        TransferFromError::TemporarilyUnavailable,
    ));

    let minter_account = Principal::anonymous().into();

    let result = retrieve_sol(
        runtime,
        minter_account,
        test_caller(),
        None,
        1,
        VALID_ADDRESS.to_string(),
    )
    .await;

    assert_eq!(
        result,
        Err(RetrieveSolError::TemporarilyUnavailable(
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

    let minter_account = Principal::anonymous().into();

    let result = retrieve_sol(
        runtime,
        minter_account,
        test_caller(),
        None,
        1,
        VALID_ADDRESS.to_string(),
    )
    .await;

    assert_eq!(
        result,
        Err(RetrieveSolError::InsufficientAllowance { allowance: 123u64 })
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

    let minter_account = Principal::anonymous().into();

    let result = retrieve_sol(
        runtime,
        minter_account,
        test_caller(),
        None,
        1,
        VALID_ADDRESS.to_string(),
    )
    .await;

    assert_eq!(
        result,
        Err(RetrieveSolError::InsufficientFunds { balance: 123u64 })
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

    let minter_account = Principal::anonymous().into();

    let result = retrieve_sol(
        runtime,
        minter_account,
        test_caller(),
        None,
        1,
        VALID_ADDRESS.to_string(),
    )
    .await;

    assert_eq!(
        result,
        Err(RetrieveSolError::GenericError {
            error_message: "msg".to_string(),
            error_code: 123u64
        })
    );
}

#[tokio::test]
async fn should_return_ok_if_burn_succeeds() {
    init_state();

    let runtime = TestCanisterRuntime::new()
        .add_stub_response(Ok::<Nat, TransferFromError>(Nat::from(123u64)));

    let minter_account = Principal::anonymous().into();

    let result = retrieve_sol(
        runtime,
        minter_account,
        test_caller(),
        None,
        1,
        VALID_ADDRESS.to_string(),
    )
    .await;

    assert_eq!(
        result,
        Ok(RetrieveSolOk {
            block_index: 123u64
        })
    );
}

#[tokio::test]
async fn should_return_error_if_address_malformed() {
    init_state();

    let runtime = TestCanisterRuntime::new();
    let minter_account = Principal::anonymous().into();

    let result = retrieve_sol(
        runtime,
        minter_account,
        test_caller(),
        None,
        1,
        "not-a-valid-address".to_string(),
    )
    .await;

    assert_matches!(result, Err(RetrieveSolError::MalformedAddress(_)));
}

#[tokio::test]
#[should_panic(expected = "the owner must be non-anonymous")]
async fn should_panic_if_caller_is_anonymous() {
    init_state();

    let runtime = TestCanisterRuntime::new();
    let minter_account = Principal::anonymous().into();

    let _ = retrieve_sol(
        runtime,
        minter_account,
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
    let _guard = retrieve_sol_guard(from).unwrap();

    let runtime = TestCanisterRuntime::new();
    let minter_account = Principal::anonymous().into();

    let result = retrieve_sol(
        runtime,
        minter_account,
        caller,
        None,
        1,
        VALID_ADDRESS.to_string(),
    )
    .await;

    assert_eq!(result, Err(RetrieveSolError::AlreadyProcessing));
}
