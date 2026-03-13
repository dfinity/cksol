use std::str::FromStr;
use std::time::Duration;

use candid::Principal;
use cksol_types::{WithdrawSolError, WithdrawSolOk};
use icrc_ledger_types::icrc1::account::{Account, Subaccount};
use icrc_ledger_types::icrc2::transfer_from::TransferFromError;
use num_traits::ToPrimitive;
use solana_address::Address;

use crate::{
    guard::{TimerGuard, withdraw_sol_guard},
    ledger::burn,
    runtime::CanisterRuntime,
    state::{
        TaskType,
        audit::process_event,
        event::{EventType, WithdrawSolRequest},
        mutate_state, read_state,
    },
};

pub const WITHDRAWAL_PROCESSING_DELAY: Duration = Duration::from_mins(1);
// The maximum number of withdrawal requests to process in a single timer invocation.
const MAX_WITHDRAWALS_PER_BATCH: usize = 10;

#[cfg(test)]
mod tests;

pub async fn withdraw_sol<R: CanisterRuntime>(
    runtime: R,
    minter_account: Account,
    caller: Principal,
    from_subaccount: Option<Subaccount>,
    amount: u64,
    address: String,
) -> Result<WithdrawSolOk, WithdrawSolError> {
    assert_ne!(
        caller,
        Principal::anonymous(),
        "the owner must be non-anonymous"
    );
    let from = Account {
        owner: caller,
        subaccount: from_subaccount,
    };
    let _guard = withdraw_sol_guard(from)?;

    let solana_address = Address::from_str(&address)
        .map_err(|e| WithdrawSolError::MalformedAddress(e.to_string()))?;

    let block_index = burn(&runtime, minter_account, from, amount, solana_address)
        .await
        .map_err(|e| match e {
            crate::ledger::BurnError::IcError(ic_error) => {
                WithdrawSolError::TemporarilyUnavailable(format!(
                    "Failed to burn tokens: {ic_error}"
                ))
            }
            crate::ledger::BurnError::TransferFromError(transfer_from_error) => {
                match transfer_from_error {
                    TransferFromError::InsufficientFunds { balance } => {
                        WithdrawSolError::InsufficientFunds {
                            balance: balance.0.to_u64().expect("balance should fit in u64"),
                        }
                    }
                    TransferFromError::InsufficientAllowance { allowance } => {
                        WithdrawSolError::InsufficientAllowance {
                            allowance: allowance.0.to_u64().expect("allowance should fit in u64"),
                        }
                    }
                    TransferFromError::TemporarilyUnavailable => {
                        WithdrawSolError::TemporarilyUnavailable(
                            "Ledger is temporarily unavailable".to_string(),
                        )
                    }
                    TransferFromError::GenericError {
                        error_code,
                        message,
                    } => WithdrawSolError::GenericError {
                        error_message: message,
                        error_code: error_code.0.to_u64().expect("error code should fit in u64"),
                    },
                    TransferFromError::BadFee { expected_fee } => {
                        panic!("Unexpected BadFee error, expected_fee: {expected_fee}")
                    }
                    TransferFromError::BadBurn { min_burn_amount } => {
                        panic!("Unexpected BadBurn error, min_burn_amount: {min_burn_amount}")
                    }
                    TransferFromError::TooOld => panic!("Unexpected TooOld error"),
                    TransferFromError::CreatedInFuture { ledger_time } => {
                        panic!("Unexpected CreatedInFuture error, ledger_time: {ledger_time}")
                    }
                    TransferFromError::Duplicate { duplicate_of } => {
                        panic!("Unexpected Duplicate error, duplicate_of: {duplicate_of}")
                    }
                }
            }
        })?;

    let withdrawal_fee = read_state(|s| s.withdrawal_fee());
    mutate_state(|s| {
        process_event(
            s,
            EventType::AcceptedWithdrawSolRequest(WithdrawSolRequest {
                account: from,
                solana_address: solana_address.to_bytes(),
                burn_block_index: block_index.into(),
                withdrawal_amount: amount,
                withdrawal_fee,
            }),
            &runtime,
        )
    });

    Ok(WithdrawSolOk { block_index })
}

pub async fn process_pending_withdrawals<R: CanisterRuntime>(_runtime: R) {
    let _guard = match TimerGuard::new(TaskType::WithdrawalProcessing) {
        Ok(guard) => guard,
        Err(_) => return,
    };

    if let Some(_pending_requests) =
        read_state(|state| state.next_pending_withdrawal_requests(MAX_WITHDRAWALS_PER_BATCH))
    {
        // TODO DEFI-2671: Build and submit withdrawal transactions
    }
}
