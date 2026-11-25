use super::*;
use crate::test_fixtures::runtime::TestCanisterRuntime;
use candid::Nat;

const LEDGER_ID: Principal = Principal::from_slice(&[1; 10]);
const AMOUNT: u64 = u64::MAX;

fn from_account() -> Account {
    Account {
        owner: Principal::from_slice(&[4; 10]),
        subaccount: Some([5; 32]),
    }
}

fn burn_memo() -> Memo {
    Memo::spl_convert(u64::MAX, solana_address::Address::from([0xFF; 32]))
}

async fn burn_tokens(runtime: &TestCanisterRuntime) -> Result<u64, SplBurnError> {
    burn(runtime, LEDGER_ID, from_account(), AMOUNT, burn_memo()).await
}

#[tokio::test]
async fn should_burn_on_token_ledger_without_deduplication() {
    let runtime = TestCanisterRuntime::new()
        .add_stub_response(Ok::<Nat, TransferFromError>(Nat::from(42_u64)));

    assert_eq!(burn_tokens(&runtime).await, Ok(42));

    let calls = runtime.sent_update_calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].canister_id, LEDGER_ID);
    assert_eq!(calls[0].method, "icrc2_transfer_from");
    assert_eq!(
        calls[0].single_arg::<TransferFromArgs>(),
        TransferFromArgs {
            spender_subaccount: None,
            from: from_account(),
            to: runtime.canister_self().into(),
            amount: AMOUNT.into(),
            fee: None,
            memo: Some(burn_memo().into()),
            created_at_time: None,
        }
    );
    let args = calls[0].single_arg::<TransferFromArgs>();
    let memo = args.memo.unwrap();
    assert_eq!(memo.0.len(), 46);
    assert_eq!(minicbor::decode::<Memo>(&memo.0).unwrap(), burn_memo());
}

#[tokio::test]
async fn should_return_a_call_error_without_retrying() {
    let error = IcError::CandidDecodeFailed {
        message: "invalid ledger reply".to_string(),
    };
    let runtime = TestCanisterRuntime::new().add_stub_error(error.clone());
    assert_eq!(burn_tokens(&runtime).await, Err(SplBurnError::Call(error)));
    assert_eq!(runtime.sent_update_calls().len(), 1);
}

#[tokio::test]
async fn should_return_expected_ledger_errors() {
    for error in [
        TransferFromError::InsufficientFunds {
            balance: 10_u64.into(),
        },
        TransferFromError::InsufficientAllowance {
            allowance: 20_u64.into(),
        },
        TransferFromError::BadBurn {
            min_burn_amount: 40_u64.into(),
        },
        TransferFromError::TemporarilyUnavailable,
        TransferFromError::GenericError {
            error_code: 60_u64.into(),
            message: "ledger error".to_string(),
        },
    ] {
        let runtime = TestCanisterRuntime::new().add_stub_response(Err::<Nat, _>(error.clone()));

        assert_eq!(
            burn_tokens(&runtime).await,
            Err(SplBurnError::TransferFrom(error))
        );
        assert_eq!(runtime.sent_update_calls().len(), 1);
    }
}

#[tokio::test]
async fn should_trap_on_unexpected_ledger_errors_like_ckerc20() {
    use futures::FutureExt;
    use std::panic::AssertUnwindSafe;
    for error in [
        TransferFromError::BadFee {
            expected_fee: 30_u64.into(),
        },
        TransferFromError::TooOld,
        TransferFromError::CreatedInFuture { ledger_time: 50 },
        TransferFromError::Duplicate {
            duplicate_of: 42_u64.into(),
        },
    ] {
        let runtime = TestCanisterRuntime::new().add_stub_response(Err::<Nat, _>(error));
        assert!(
            AssertUnwindSafe(burn_tokens(&runtime))
                .catch_unwind()
                .await
                .is_err()
        );
        assert_eq!(runtime.sent_update_calls().len(), 1);
    }
}
