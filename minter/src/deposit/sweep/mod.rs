use crate::utils::assert_non_anonymous_account;
use cksol_types::{DepositSolError, DepositSolId, DepositSolStatus};
use icrc_ledger_types::icrc1::account::Account;

#[cfg(test)]
mod tests;

const PLACEHOLDER_DEPOSIT_ID: DepositSolId = 0;

pub fn deposit_sol(account: Account) -> Result<DepositSolId, DepositSolError> {
    assert_non_anonymous_account(&account);
    Ok(PLACEHOLDER_DEPOSIT_ID)
}

pub fn deposit_status(_deposit_id: DepositSolId) -> DepositSolStatus {
    DepositSolStatus::Queued {
        sweepable_amount: 0,
    }
}
