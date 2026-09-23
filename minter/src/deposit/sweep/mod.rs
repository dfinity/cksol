use cksol_types::{DepositSolError, DepositSolStatus};
use icrc_ledger_types::icrc1::account::Account;

#[cfg(test)]
mod tests;

pub fn deposit_sol(account: Account) -> Result<DepositSolStatus, DepositSolError> {
    Ok(queued_with_nothing_to_sweep(account))
}

pub fn deposit_status(account: Account) -> Option<DepositSolStatus> {
    Some(queued_with_nothing_to_sweep(account))
}

fn queued_with_nothing_to_sweep(account: Account) -> DepositSolStatus {
    DepositSolStatus::Queued {
        account,
        sweepable_amount: 0,
    }
}
