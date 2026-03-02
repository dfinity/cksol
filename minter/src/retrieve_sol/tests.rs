use crate::{retrieve_sol::retrieve_sol, runtime::TestCanisterRuntime, test_fixtures::init_state};
use assert_matches::assert_matches;
use candid::{Nat, Principal};
use cksol_types::{RetrieveSolError, RetrieveSolOk};
use ic_canister_runtime::IcError;
use icrc_ledger_types::icrc2::transfer_from::TransferFromError;
use solana_address::Address;

#[tokio::test]
async fn should_return_error_if_calling_ledger_fails() {
    init_state();

    let runtime = TestCanisterRuntime::new().add_stub_error(IcError::CallPerformFailed);

    let account = Principal::anonymous().into();

    let result = retrieve_sol(runtime, account, account, 1, Address::default()).await;

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

    let account = Principal::anonymous().into();

    let result = retrieve_sol(runtime, account, account, 1, Address::default()).await;

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

    let account = Principal::anonymous().into();

    let result = retrieve_sol(runtime, account, account, 1, Address::default()).await;

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

    let account = Principal::anonymous().into();

    let result = retrieve_sol(runtime, account, account, 1, Address::default()).await;

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

    let account = Principal::anonymous().into();

    let result = retrieve_sol(runtime, account, account, 1, Address::default()).await;

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

    let account = Principal::anonymous().into();

    let result = retrieve_sol(runtime, account, account, 1, Address::default()).await;

    assert_eq!(
        result,
        Ok(RetrieveSolOk {
            block_index: 123u64
        })
    );
}
