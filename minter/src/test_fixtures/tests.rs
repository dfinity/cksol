use super::signer::sign_for;
use super::{
    MINTER_ACCOUNT, account, account_signature, account_signature_nth, minter_signature, signature,
    signer::MockSchnorrSigner,
};
use crate::{address::derivation_path, signer::SchnorrSigner};
use candid::Principal;
use ic_cdk::call::CallRejected;
use ic_cdk_management_canister::SignCallError;
use icrc_ledger_types::icrc1::account::Account;
use solana_signature::Signature;

#[test]
fn should_derive_distinct_signatures_for_distinct_accounts() {
    let accounts = [
        MINTER_ACCOUNT,
        Account::from(Principal::from_slice(&[1])),
        Account::from(Principal::from_slice(&[1, 0])),
        Account::from(Principal::from_slice(&[1, 0, 0])),
        Account::from(Principal::from_slice(&[0])),
        account(0),
        account(1),
        Account::from(Principal::from_slice(&[1; 29])),
        Account {
            owner: Principal::from_slice(&[1]),
            subaccount: Some([3; 32]),
        },
        Account {
            owner: Principal::from_slice(&[1; 29]),
            subaccount: Some([3; 32]),
        },
    ];

    for (index, account) in accounts.iter().enumerate() {
        assert_ne!(minter_signature(), account_signature(account));
        for other in &accounts[index + 1..] {
            assert_ne!(account_signature(account), account_signature(other));
        }
    }
}

#[test]
fn should_derive_distinct_signatures_for_each_occurrence() {
    let occurrences: Vec<_> = (0..8)
        .map(|occurrence| account_signature_nth(&account(1), occurrence))
        .collect();

    for (index, signature) in occurrences.iter().enumerate() {
        for other in &occurrences[index + 1..] {
            assert_ne!(signature, other);
        }
    }
}

#[test]
fn should_derive_the_first_occurrence_by_default() {
    assert_eq!(
        account_signature(&account(1)),
        account_signature_nth(&account(1), 0)
    );
}

#[test]
fn should_ignore_a_default_subaccount() {
    let account = Account::from(Principal::from_slice(&[1]));

    assert_eq!(
        account_signature(&account),
        account_signature(&Account {
            subaccount: Some([0; 32]),
            ..account
        })
    );
}

#[tokio::test]
async fn should_answer_a_registered_request_with_the_derived_signature() {
    let signer = MockSchnorrSigner::default()
        .add_signer(sign_for(&account(0)))
        .add_signer(sign_for(&account(1)));

    assert_eq!(
        sign(&signer, &account(0)).await,
        account_signature(&account(0))
    );
    assert_eq!(
        sign(&signer, &account(1)).await,
        account_signature(&account(1))
    );
}

#[tokio::test]
async fn should_answer_a_registered_request_with_the_given_signature() {
    let signer = MockSchnorrSigner::default()
        .add_signer(sign_for(&account(1)).expect([Ok(signature(0xAA))]));

    assert_eq!(sign(&signer, &account(1)).await, signature(0xAA));
}

#[tokio::test]
async fn should_advance_the_occurrence_per_account() {
    let signer = MockSchnorrSigner::default()
        .add_signer(sign_for(&account(1)))
        .add_signer(sign_for(&account(2)))
        .add_signer(sign_for(&account(1)));

    assert_eq!(
        sign(&signer, &account(1)).await,
        account_signature(&account(1))
    );
    assert_eq!(
        sign(&signer, &account(2)).await,
        account_signature(&account(2))
    );
    assert_eq!(
        sign(&signer, &account(1)).await,
        account_signature_nth(&account(1), 1)
    );
}

#[tokio::test]
async fn should_answer_registrations_for_one_account_in_order() {
    let signer = MockSchnorrSigner::default()
        .add_signer(sign_for(&account(1)).expect([Ok(signature(0xAA))]))
        .add_signer(sign_for(&account(1)))
        .add_signer(sign_for(&account(1)).expect([Ok(signature(0xBB))]));

    assert_eq!(sign(&signer, &account(1)).await, signature(0xAA));
    assert_eq!(
        sign(&signer, &account(1)).await,
        account_signature_nth(&account(1), 1)
    );
    assert_eq!(sign(&signer, &account(1)).await, signature(0xBB));
}

#[tokio::test]
async fn should_share_consumed_registrations_between_clones() {
    let signer = MockSchnorrSigner::default()
        .add_signer(sign_for(&account(1)).expect([Ok(signature(0xAA))]))
        .add_signer(sign_for(&account(1)));
    let clone = signer.clone();

    assert_eq!(sign(&signer, &account(1)).await, signature(0xAA));
    assert_eq!(
        sign(&clone, &account(1)).await,
        account_signature_nth(&account(1), 1)
    );
}

#[tokio::test]
async fn should_fail_the_registered_request_only() {
    let signer = MockSchnorrSigner::default()
        .add_signer(sign_for(&account(1)).expect([Err(signing_error())]))
        .add_signer(sign_for(&account(1)));

    assert!(
        signer
            .sign(vec![], derivation_path(&account(1)))
            .await
            .is_err()
    );
    assert_eq!(
        sign(&signer, &account(1)).await,
        account_signature_nth(&account(1), 1)
    );
}

#[tokio::test]
#[should_panic(expected = "No matching expectation found")]
async fn should_panic_on_an_unregistered_signing_request() {
    let signer = MockSchnorrSigner::default().add_signer(sign_for(&account(1)));

    sign(&signer, &account(2)).await;
}

#[tokio::test]
#[should_panic(expected = "fewer than expected")]
async fn should_panic_on_a_registration_that_is_never_used() {
    let signer = MockSchnorrSigner::default()
        .add_signer(sign_for(&account(1)))
        .add_signer(sign_for(&account(2)));

    sign(&signer, &account(1)).await;
}

#[tokio::test]
#[should_panic(expected = "register all expected signers")]
async fn should_panic_on_a_signer_added_after_the_first_signing_request() {
    let signer = MockSchnorrSigner::default().add_signer(sign_for(&account(1)));
    sign(&signer, &account(1)).await;

    let _ = signer.add_signer(sign_for(&account(1)));
}

async fn sign(signer: &MockSchnorrSigner, account: &Account) -> Signature {
    let bytes = signer
        .sign(vec![], derivation_path(account))
        .await
        .expect("signing should succeed");
    Signature::try_from(bytes.as_slice()).expect("expected a 64-byte signature")
}

fn signing_error() -> SignCallError {
    SignCallError::CallFailed(CallRejected::with_rejection(4, "unavailable".to_string()).into())
}

#[test]
#[should_panic(expected = "never requested")]
fn should_panic_on_an_expected_signer_that_never_signs() {
    let _signer = MockSchnorrSigner::default().add_signer(sign_for(&account(1)).times(2));
}

#[tokio::test]
#[should_panic(expected = "fewer than expected")]
async fn should_panic_when_an_account_signs_fewer_times_than_expected() {
    let signer = MockSchnorrSigner::default().add_signer(sign_for(&account(1)).times(2));

    sign(&signer, &account(1)).await;
}

#[tokio::test]
#[should_panic(expected = "No matching expectation found")]
async fn should_panic_when_an_account_signs_more_times_than_expected() {
    let signer = MockSchnorrSigner::default().add_signer(sign_for(&account(1)).times(2));

    sign(&signer, &account(1)).await;
    sign(&signer, &account(1)).await;
    sign(&signer, &account(1)).await;
}

#[tokio::test]
#[should_panic(expected = "fewer than expected")]
async fn should_panic_when_a_given_sequence_is_not_exhausted() {
    let signer = MockSchnorrSigner::default()
        .add_signer(sign_for(&account(1)).expect([Ok(signature(0xAA)), Ok(signature(0xBB))]));

    assert_eq!(sign(&signer, &account(1)).await, signature(0xAA));
}

mod inter_canister_calls {
    use crate::{
        runtime::CanisterRuntime,
        test_fixtures::{ledger_canister_id, runtime::TestCanisterRuntime},
    };
    use candid::Nat;
    use ic_canister_runtime::{IcError, Runtime};
    use icrc_ledger_types::{
        icrc1::{account::Account, transfer::BlockIndex},
        icrc2::transfer_from::{TransferFromArgs, TransferFromError},
    };
    use sol_rpc_types::MultiRpcResult;

    type TransferFromResult = Result<BlockIndex, TransferFromError>;

    #[tokio::test]
    async fn should_answer_the_call_matching_an_expectation() {
        let runtime = TestCanisterRuntime::new()
            .expect_icrc2_transfer_from(transfer_from_args(7), Ok(BlockIndex::from(42_u64)));

        let response: TransferFromResult = transfer_from(&runtime, transfer_from_args(7))
            .await
            .expect("the call should be answered");

        assert_eq!(response, Ok(BlockIndex::from(42_u64)));
    }

    #[tokio::test]
    async fn should_fail_the_call_with_the_expected_error() {
        let runtime = TestCanisterRuntime::new()
            .expect_icrc2_transfer_from(transfer_from_args(7), IcError::CallPerformFailed);

        let response = transfer_from(&runtime, transfer_from_args(7)).await;

        assert_eq!(response, Err(IcError::CallPerformFailed));
    }

    #[tokio::test]
    #[should_panic(expected = "No matching expectation found")]
    async fn should_panic_on_a_call_without_a_matching_expectation() {
        let runtime = TestCanisterRuntime::new();

        let _ = transfer_from(&runtime, transfer_from_args(7)).await;
    }

    #[tokio::test]
    #[should_panic(expected = "No matching expectation found")]
    async fn should_panic_on_a_call_with_unexpected_arguments() {
        let runtime = TestCanisterRuntime::new()
            .expect_icrc2_transfer_from(transfer_from_args(7), Ok(BlockIndex::from(42_u64)));

        let _ = transfer_from(&runtime, transfer_from_args(8)).await;
    }

    #[test]
    #[should_panic(expected = "fewer than expected")]
    fn should_panic_on_an_expectation_that_is_never_called() {
        let runtime =
            TestCanisterRuntime::new().expect_get_slot(MultiRpcResult::Consistent(Ok(42)));

        drop(runtime);
    }

    async fn transfer_from(
        runtime: &TestCanisterRuntime,
        args: TransferFromArgs,
    ) -> Result<TransferFromResult, IcError> {
        runtime
            .inter_canister_call_runtime()
            .update_call(ledger_canister_id(), "icrc2_transfer_from", (args,), 0)
            .await
    }

    fn transfer_from_args(amount: u64) -> TransferFromArgs {
        TransferFromArgs {
            spender_subaccount: None,
            from: Account::from(candid::Principal::from_slice(&[1])),
            to: Account::from(candid::Principal::from_slice(&[2])),
            fee: None,
            created_at_time: None,
            memo: None,
            amount: Nat::from(amount),
        }
    }
}
