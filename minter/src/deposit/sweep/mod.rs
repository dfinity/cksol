use cksol_types::{DepositSolError, DepositSolId, DepositSolStatus};
use icrc_ledger_types::icrc1::account::Account;

#[cfg(test)]
mod tests;

const PLACEHOLDER_DEPOSIT_ID: DepositSolId = DepositSolId::new(0);

pub fn deposit_sol(_account: Account) -> Result<DepositSolId, DepositSolError> {
    Ok(PLACEHOLDER_DEPOSIT_ID)
}

pub fn deposit_status(_deposit_id: DepositSolId) -> Option<DepositSolStatus> {
    Some(DepositSolStatus::Queued {
        sweepable_amount: 0,
    })
}
