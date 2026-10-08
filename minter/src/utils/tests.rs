use crate::utils::{assert_non_anonymous_account, assert_valid_deposit_owner};
use candid::Principal;
use icrc_ledger_types::icrc1::account::Account;

const MINTER_ID: Principal = Principal::from_slice(&[0xCA; 10]);

fn account_of(owner: Principal) -> Account {
    Account {
        owner,
        subaccount: None,
    }
}

#[test]
fn should_accept_regular_deposit_owner() {
    assert_valid_deposit_owner(&account_of(Principal::from_slice(&[1, 2, 3])), MINTER_ID);
}

#[test]
#[should_panic(expected = "the owner must be non-anonymous")]
fn should_reject_anonymous_deposit_owner() {
    assert_valid_deposit_owner(&account_of(Principal::anonymous()), MINTER_ID);
}

#[test]
#[should_panic(expected = "is not a valid deposit owner")]
fn should_reject_the_minter_as_deposit_owner() {
    assert_valid_deposit_owner(&account_of(MINTER_ID), MINTER_ID);
}

#[test]
#[should_panic(expected = "the owner must be non-anonymous")]
fn should_reject_anonymous_account() {
    assert_non_anonymous_account(&account_of(Principal::anonymous()));
}
