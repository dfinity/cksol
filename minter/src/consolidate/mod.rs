use crate::{
    guard::TimerGuard,
    runtime::CanisterRuntime,
    sol_transfer::{CreateTransferError, MAX_SIGNATURES, create_signed_transfer_transaction},
    state::{TaskType, audit::process_event, event::EventType, mutate_state, read_state},
    transaction::{SubmitTransactionError, get_recent_blockhash, get_slot, submit_transaction},
};
use canlog::log;
use cksol_types_internal::log::Priority;
use icrc_ledger_types::icrc1::account::Account;
use sol_rpc_types::{Lamport, Slot};
use solana_hash::Hash;
use solana_signature::Signature;
use std::time::Duration;
use thiserror::Error;

#[cfg(test)]
mod tests;

pub const DEPOSIT_CONSOLIDATION_DELAY: Duration = Duration::from_mins(10);
const MAX_CONCURRENT_TRANSACTIONS: usize = 10;
const MAX_TRANSFERS_PER_CONSOLIDATION: usize = MAX_SIGNATURES as usize - 1;

pub async fn consolidate_deposits<R: CanisterRuntime>(runtime: R) {
    let _guard = match TimerGuard::new(TaskType::DepositConsolidation) {
        Ok(guard) => guard,
        Err(_) => return,
    };

    if read_state(|state| state.funds_to_consolidate().is_empty()) {
        return;
    }

    let funds_to_consolidate: Vec<_> = read_state(|state| {
        state
            .funds_to_consolidate()
            .clone()
            .into_iter()
            .collect::<Vec<_>>()
            .chunks(MAX_TRANSFERS_PER_CONSOLIDATION)
            .map(|c| c.to_vec())
            .collect()
    });

    for round in funds_to_consolidate.chunks(MAX_CONCURRENT_TRANSACTIONS) {
        let recent_blockhash = match get_recent_blockhash(&runtime).await {
            Ok(blockhash) => blockhash,
            Err(e) => {
                log!(Priority::Info, "Failed to fetch recent blockhash: {e}");
                return;
            }
        };
        // TODO DEFI-2670: Update `sol_rpc_client` to return the slot along with the blockhash
        //  in `estimate_recent_blockhash`, then remove this separate call to `getSlot`.
        let slot = match get_slot(&runtime).await {
            Ok(slot) => slot,
            Err(e) => {
                log!(Priority::Info, "Failed to fetch slot: {e}");
                return;
            }
        };
        let _ = futures::future::join_all(round.iter().cloned().map(|funds| {
            try_submit_consolidation_transaction(runtime.clone(), funds, slot, recent_blockhash)
        }))
        .await;
    }
}

async fn try_submit_consolidation_transaction<R: CanisterRuntime>(
    runtime: R,
    funds_to_consolidate: Vec<(Account, Lamport)>,
    slot: Slot,
    recent_blockhash: Hash,
) -> Option<Signature> {
    match submit_consolidation_transaction(&runtime, funds_to_consolidate, slot, recent_blockhash)
        .await
    {
        Ok(signature) => Some(signature),
        Err(e) => {
            log!(Priority::Info, "Deposit consolidation failed: {e}");
            None
        }
    }
}

#[derive(Debug, Error)]
enum ConsolidationError {
    #[error("failed to create transaction: {0}")]
    CreateTransactionFailed(#[from] CreateTransferError),
    #[error("failed to submit transaction: {0}")]
    SubmitTransactionFailed(#[from] SubmitTransactionError),
}

async fn submit_consolidation_transaction<R: CanisterRuntime>(
    runtime: &R,
    funds_to_consolidate: Vec<(Account, Lamport)>,
    slot: Slot,
    recent_blockhash: Hash,
) -> Result<Signature, ConsolidationError> {
    let minter_account = Account {
        owner: runtime.canister_self(),
        subaccount: None,
    };
    let (transaction, signers) = create_signed_transfer_transaction(
        minter_account,
        &funds_to_consolidate,
        minter_account,
        recent_blockhash,
        &runtime.signer(),
    )
    .await?;

    let signature = transaction.signatures[0];
    let message = transaction.message.clone();

    // Record events before trying to submit the transaction to ensure we don't
    // resubmit the same transaction twice in case submission fails.
    mutate_state(|state| {
        process_event(
            state,
            EventType::ConsolidatedDeposits {
                deposits: funds_to_consolidate,
            },
            runtime,
        )
    });
    mutate_state(|state| {
        process_event(
            state,
            EventType::SubmittedTransaction {
                signature,
                transaction: message,
                signers,
                slot,
            },
            runtime,
        )
    });

    submit_transaction(runtime, transaction).await?;

    Ok(signature)
}
