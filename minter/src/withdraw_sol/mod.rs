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

use crate::{
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
    transaction::{get_recent_blockhash, get_slot},
};

pub const WITHDRAWAL_PROCESSING_DELAY: Duration = Duration::from_mins(1);
pub(crate) const MAX_CONCURRENT_WITHDRAWAL_TXS: usize = 10;

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

    let max_requests = MAX_WITHDRAWALS_PER_TX * MAX_CONCURRENT_WITHDRAWAL_TXS;
    let withdrawal_batches: Vec<Vec<_>> = read_state(|state| {
        state
            .pending_withdrawal_requests()
            .values()
            .take(max_requests)
            .cloned()
            .collect::<Vec<_>>()
    })
    .into_iter()
    .chunks(MAX_WITHDRAWALS_PER_TX)
    .into_iter()
    .map(Iterator::collect)
    .collect();

    if withdrawal_batches.is_empty() {
        return;
    }

    // TODO: we need to check whether the minter has enough funds in the main account.
    // We probably need to add a state.minter_balance variable and update it
    // here and while consolidating funds.
    // If there are not enough funds for the withdrawal we simply continue.

    let recent_blockhash = match get_recent_blockhash(runtime).await {
        Ok(blockhash) => blockhash,
        Err(e) => {
            log!(Priority::Info, "Failed to fetch recent blockhash: {e}");
            return;
        }
    };
    // TODO DEFI-2670: Update `sol_rpc_client` to return the slot along with the blockhash
    //  in `estimate_recent_blockhash`, then remove this separate call to `getSlot`.
    let slot = match get_slot(runtime).await {
        Ok(slot) => slot,
        Err(e) => {
            log!(Priority::Info, "Failed to get slot: {e}");
            return;
        }
    };

    futures::future::join_all(withdrawal_batches.into_iter().map(async |batch| {
        submit_withdrawal_transaction(runtime, batch, slot, recent_blockhash).await
    }))
    .await;
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
    let message = VersionedMessage::Legacy(signed_tx.message);
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

    // TODO: Send the transaction to the Solana network via RPC.
}

pub fn withdraw_sol_status(block_index: u64) -> WithdrawSolStatus {
    read_state(|s| s.withdrawal_status(block_index))
}
