use std::str::FromStr;
use std::time::Duration;

use candid::Principal;
use cksol_types::{WithdrawSolError, WithdrawSolOk, WithdrawSolStatus};
use icrc_ledger_types::icrc1::account::{Account, Subaccount};
use icrc_ledger_types::icrc2::transfer_from::TransferFromError;
use num_traits::ToPrimitive;
use solana_address::Address;

use canlog::log;
use cksol_types_internal::log::Priority;

use itertools::Itertools;
use sol_rpc_types::Slot;
use solana_hash::Hash;

use crate::constants::MAX_CONCURRENT_RPC_CALLS;
use crate::{
    consolidate::consolidate_deposits,
    guard::{TimerGuard, withdraw_sol_guard},
    ledger::burn,
    runtime::CanisterRuntime,
    sol_transfer::{MAX_WITHDRAWALS_PER_TX, create_signed_batch_withdrawal_transaction},
    state::{
        TaskType,
        audit::process_event,
        event::{EventType, TransactionPurpose, VersionedMessage, WithdrawSolRequest},
        mutate_state, read_state,
    },
    transaction::{get_recent_slot_and_blockhash, submit_transaction},
};

pub const WITHDRAWAL_PROCESSING_DELAY: Duration = Duration::from_mins(1);
pub(crate) const MAX_WITHDRAWAL_ROUNDS: usize = 5;

#[cfg(test)]
mod tests;

pub async fn withdraw_sol<R: CanisterRuntime>(
    runtime: &R,
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

    let block_index = burn(runtime, minter_account, from, amount, solana_address)
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
            runtime,
        )
    });

    Ok(WithdrawSolOk { block_index })
}

pub async fn process_pending_withdrawals<R: CanisterRuntime>(runtime: &R) {
    let _guard = match TimerGuard::new(TaskType::WithdrawalProcessing) {
        Ok(guard) => guard,
        Err(_) => {
            log!(
                Priority::Info,
                "failed to obtain WithdrawalProcessing guard, exiting"
            );
            return;
        }
    };

    let max_per_invocation =
        MAX_WITHDRAWAL_ROUNDS * MAX_CONCURRENT_RPC_CALLS * MAX_WITHDRAWALS_PER_TX;

    let (affordable_requests, has_unaffordable) = read_state(|state| {
        let mut available = state.balance();
        let mut affordable = Vec::new();
        let mut has_unaffordable = false;

        for request in state
            .pending_withdrawal_requests()
            .values()
            .take(max_per_invocation)
        {
            let transfer_amount = request
                .withdrawal_amount
                .checked_sub(request.withdrawal_fee)
                .expect("BUG: withdrawal_amount must be >= withdrawal_fee");
            if available >= transfer_amount {
                available -= transfer_amount;
                affordable.push(request.clone());
            } else {
                has_unaffordable = true;
                break;
            }
        }
        (affordable, has_unaffordable)
    });

    if has_unaffordable {
        log!(
            Priority::Info,
            "Insufficient minter balance for some withdrawal requests, scheduling consolidation"
        );
        let runtime_clone = runtime.clone();
        runtime.set_timer(Duration::ZERO, async move {
            consolidate_deposits(runtime_clone).await;
        });
    }

    if affordable_requests.is_empty() {
        return;
    }

    let rounds: Vec<Vec<Vec<_>>> = affordable_requests
        .into_iter()
        .chunks(MAX_WITHDRAWALS_PER_TX)
        .into_iter()
        .map(Iterator::collect)
        .collect::<Vec<Vec<_>>>()
        .into_iter()
        .chunks(MAX_CONCURRENT_RPC_CALLS)
        .into_iter()
        .take(MAX_WITHDRAWAL_ROUNDS)
        .map(Iterator::collect)
        .collect();

    for round in rounds {
        let (slot, recent_blockhash) = match get_recent_slot_and_blockhash(runtime).await {
            Ok((slot, blockhash)) => (slot, blockhash),
            Err(e) => {
                log!(Priority::Info, "Failed to fetch recent blockhash: {e}");
                return;
            }
        };

        futures::future::join_all(round.into_iter().map(async |batch| {
            submit_withdrawal_transaction(runtime, batch, slot, recent_blockhash).await
        }))
        .await;
    }
}

async fn submit_withdrawal_transaction<R: CanisterRuntime>(
    runtime: &R,
    requests: Vec<WithdrawSolRequest>,
    slot: Slot,
    recent_blockhash: Hash,
) {
    let targets: Vec<_> = requests
        .iter()
        .map(|request| {
            let destination = Address::from(request.solana_address);
            let transfer_amount = request
                .withdrawal_amount
                .checked_sub(request.withdrawal_fee)
                .expect("BUG: withdrawal_amount must be >= withdrawal_fee");
            (destination, transfer_amount)
        })
        .collect();

    let (signed_tx, signers) = match create_signed_batch_withdrawal_transaction(
        runtime,
        &targets,
        recent_blockhash,
    )
    .await
    {
        Ok(tx) => tx,
        Err(e) => {
            let burn_indices: Vec<_> = requests.iter().map(|r| r.burn_block_index).collect();
            log!(
                Priority::Error,
                "Failed to create batch withdrawal transaction for burn indices {burn_indices:?}: {e}"
            );
            return;
        }
    };

    let signature = signed_tx.signatures[0];
    let message = VersionedMessage::Legacy(signed_tx.message.clone());
    let burn_indices = requests.iter().map(|r| r.burn_block_index).collect();

    mutate_state(|state| {
        process_event(
            state,
            EventType::SubmittedTransaction {
                signature,
                message,
                signers,
                slot,
                purpose: TransactionPurpose::WithdrawSol { burn_indices },
            },
            runtime,
        )
    });

    let _ = submit_transaction(runtime, signed_tx).await;
}

pub fn withdraw_sol_status(block_index: u64) -> WithdrawSolStatus {
    read_state(|s| s.withdrawal_status(block_index))
}
