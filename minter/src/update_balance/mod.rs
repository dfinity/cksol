use cksol_types::{DepositStatus, UpdateBalanceError};
use icrc_ledger_types::icrc1::account::Account;

pub async fn update_balance(
    _account: Account,
    _signature: solana_signature::Signature,
) -> Result<DepositStatus, UpdateBalanceError> {
    Err(UpdateBalanceError::TemporarilyUnavailable(
        "Not yet implemented!".to_string(),
    ))
}
