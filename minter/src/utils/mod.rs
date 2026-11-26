use crate::state::{SupportedSplToken, read_state};
use candid::Principal;
use cksol_types::DepositSplError;
use icrc_ledger_types::icrc1::account::Account;
use solana_address::Address;

pub mod insertion_ordered_map;
pub mod timed_sized_map;

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

pub fn assert_valid_deposit_token(mint: &Address) -> Result<SupportedSplToken, DepositSplError> {
    read_state(|state| {
        let token =
            state
                .supported_spl_token(mint)
                .ok_or_else(|| DepositSplError::UnsupportedToken {
                    mint: (*mint).into(),
                })?;
        if token.deposits_paused {
            return Err(DepositSplError::TokenPaused {
                mint: (*mint).into(),
            });
        }
        Ok(token.clone())
    })
}
