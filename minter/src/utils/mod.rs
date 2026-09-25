use candid::Principal;
use icrc_ledger_types::icrc1::account::Account;

pub mod insertion_ordered_map;

pub fn assert_non_anonymous_account(account: &Account) {
    assert_ne!(
        account.owner,
        Principal::anonymous(),
        "the owner must be non-anonymous"
    );
}
