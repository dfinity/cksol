use crate::{
    guard::TimerGuard,
    runtime::CanisterRuntime,
    sol_transfer::{
        CreateTransferError, IcSchnorrSigner, MAX_SIGNATURES, create_signed_transfer_transaction,
    },
    state::{TaskType, audit::process_event, event::EventType, mutate_state, read_state},
    transaction::{SubmitTransactionError, get_recent_blockhash, submit_transaction},
};
use canlog::log;
use cksol_types_internal::log::Priority;
use icrc_ledger_types::icrc1::account::Account;
use sol_rpc_types::Lamport;
use solana_hash::Hash;
use solana_signature::Signature;
use std::time::Duration;
use thiserror::Error;

pub const DEPOSIT_CONSOLIDATION_DELAY: Duration = Duration::from_mins(10);

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
            // Need to account for fee payer signature
            .chunks(MAX_SIGNATURES as usize - 1)
            .map(|c| c.to_vec())
            .collect()
    });

    let recent_blockhash = match get_recent_blockhash(&runtime).await {
        Ok(blockhash) => blockhash,
        Err(e) => {
            log!(Priority::Info, "Failed to fetch recent blockhash: {e}");
            return;
        }
    };

    let _ = futures::future::join_all(funds_to_consolidate.into_iter().map(|funds| {
        try_submit_consolidation_transaction(runtime.clone(), funds, recent_blockhash)
    }))
    .await;
}

async fn try_submit_consolidation_transaction<R: CanisterRuntime>(
    runtime: R,
    funds_to_consolidate: Vec<(Account, Lamport)>,
    recent_blockhash: Hash,
) -> Option<Signature> {
    match submit_consolidation_transaction(&runtime, funds_to_consolidate, recent_blockhash).await {
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
    recent_blockhash: Hash,
) -> Result<Signature, ConsolidationError> {
    let minter_account = Account {
        owner: ic_cdk::api::canister_self(),
        subaccount: None,
    };
    let transaction = create_signed_transfer_transaction(
        minter_account,
        &funds_to_consolidate,
        minter_account,
        recent_blockhash,
        &IcSchnorrSigner,
    )
    .await?;

    let signature = submit_transaction(runtime, transaction.clone()).await?;

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
                transaction: transaction.message,
            },
            runtime,
        )
    });

    Ok(signature)
}
