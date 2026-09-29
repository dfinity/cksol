use candid::Principal;
use icrc_ledger_types::icrc1::account::Account;

pub mod insertion_ordered_map;

#[cfg(test)]
mod tests;

pub fn assert_non_anonymous_account(account: &Account) {
    assert_ne!(
        account.owner,
        Principal::anonymous(),
        "the owner must be non-anonymous"
    );
}

pub fn assert_valid_deposit_owner(account: &Account, minter_id: Principal) {
    assert_non_anonymous_account(account);
    assert_ne!(
        account.owner, minter_id,
        "the minter's own principal {minter_id} is not a valid deposit owner"
    );
}
