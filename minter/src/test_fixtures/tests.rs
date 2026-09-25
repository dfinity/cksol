use super::signer::ExpectedSignature::{Derived, Exactly, Failing};
use super::{
    MINTER_ACCOUNT, account, account_signature, account_signature_nth, signature,
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
        .add_signature(&account(0), Derived)
        .add_signature(&account(1), Derived);

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
    let signer = MockSchnorrSigner::default().add_signature(&account(1), Exactly(signature(0xAA)));

    assert_eq!(sign(&signer, &account(1)).await, signature(0xAA));
}

#[tokio::test]
async fn should_advance_the_occurrence_per_account() {
    let signer = MockSchnorrSigner::default()
        .add_signature(&account(1), Derived)
        .add_signature(&account(2), Derived)
        .add_signature(&account(1), Derived);

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
        .add_signature(&account(1), Exactly(signature(0xAA)))
        .add_signature(&account(1), Derived)
        .add_signature(&account(1), Exactly(signature(0xBB)));

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
        .add_signature(&account(1), Exactly(signature(0xAA)))
        .add_signature(&account(1), Derived);
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
        .add_signature(&account(1), Failing(signing_error()))
        .add_signature(&account(1), Derived);

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
    let signer = MockSchnorrSigner::default().add_signature(&account(1), Derived);

    sign(&signer, &account(2)).await;
}

#[tokio::test]
#[should_panic(expected = "fewer than expected")]
async fn should_panic_on_a_registration_that_is_never_used() {
    let signer = MockSchnorrSigner::default()
        .add_signature(&account(1), Derived)
        .add_signature(&account(2), Derived);

    sign(&signer, &account(1)).await;
}

#[tokio::test]
#[should_panic(expected = "register all expected signatures")]
async fn should_panic_on_a_registration_added_after_the_first_signing_request() {
    let signer = MockSchnorrSigner::default().add_signature(&account(1), Derived);
    sign(&signer, &account(1)).await;

    let _ = signer.add_signature(&account(1), Derived);
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
