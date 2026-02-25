use cksol_types::{Address, RetrieveSolError, RetrieveSolOk};
use icrc_ledger_types::{icrc1::account::Account, icrc2::transfer_from::TransferFromError};
use num_traits::ToPrimitive;

use crate::{ledger::burn, runtime::CanisterRuntime};

pub async fn retrieve_sol<R: CanisterRuntime>(
    runtime: R,
    from: Account,
    amount: u64,
    to: Address,
) -> Result<RetrieveSolOk, RetrieveSolError> {
    let block_index = burn(&runtime, from, amount, to)
        .await
        .map_err(|e| match e {
            crate::ledger::BurnError::IcError(ic_error) => {
                RetrieveSolError::TemporarilyUnavailable(format!(
                    "Failed to burn tokens: {ic_error}"
                ))
            }
            crate::ledger::BurnError::TransferFromError(transfer_from_error) => {
                match transfer_from_error {
                    TransferFromError::InsufficientFunds { balance } => {
                        RetrieveSolError::InsufficientFunds {
                            balance: balance.0.to_u64().expect("balance should fit in u64"),
                        }
                    }
                    TransferFromError::InsufficientAllowance { allowance } => {
                        RetrieveSolError::InsufficientAllowance {
                            allowance: allowance.0.to_u64().expect("allowance should fit in u64"),
                        }
                    }
                    TransferFromError::Duplicate { duplicate_of: _ } => {
                        RetrieveSolError::AlreadyProcessing
                    }
                    TransferFromError::TemporarilyUnavailable => {
                        RetrieveSolError::TemporarilyUnavailable(
                            "Ledger is temporarily unavailable".to_string(),
                        )
                    }
                    TransferFromError::GenericError {
                        error_code,
                        message,
                    } => RetrieveSolError::GenericError {
                        error_message: message,
                        error_code: error_code.0.to_u64().expect("error code should fit in u64"),
                    },
                    other_error => panic!("Unexpected burn error: {other_error}"),
                }
            }
        })?;
    Ok(RetrieveSolOk { block_index })
}
