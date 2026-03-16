use crate::{
    guard::TimerGuard,
    runtime::CanisterRuntime,
    sol_transfer::{
        CreateTransferError, IcSchnorrSigner, MAX_SIGNATURES, create_signed_transfer_transaction,
    },
    state::{TaskType, audit::process_event, event::EventType, mutate_state, read_state},
};
use canlog::log;
use cksol_types_internal::log::Priority;
use ic_canister_runtime::Runtime;
use icrc_ledger_types::icrc1::account::Account;
use sol_rpc_client::SolRpcClient;
use sol_rpc_types::{Lamport, MultiRpcResult};
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

    // TODO DEFI-2670: Use `try_send` to avoid panic in case of `IcError`
    let recent_blockhash = match sol_rpc_client(&runtime)
        .estimate_recent_blockhash()
        .send()
        .await
    {
        Ok(blockhash) => blockhash,
        Err(e) => {
            log!(Priority::Info, "Failed to fetch recent blockhash: {e:?}");
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

async fn submit_consolidation_transaction<R: CanisterRuntime>(
    runtime: &R,
    funds_to_consolidate: Vec<(Account, Lamport)>,
    recent_blockhash: Hash,
) -> Result<Signature, SubmitTransactionError> {
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

    let signature = match sol_rpc_client(runtime)
        .send_transaction(transaction.clone())
        .try_send()
        .await
    {
        Ok(MultiRpcResult::Consistent(Ok(signature))) => signature,
        Ok(MultiRpcResult::Consistent(Err(e))) => {
            return Err(SubmitTransactionError::SendTransactionCallFailed(
                e.to_string(),
            ));
        }
        Ok(MultiRpcResult::Inconsistent(_)) => {
            return Err(SubmitTransactionError::SendTransactionConsensusError);
        }
        Err(e) => {
            return Err(SubmitTransactionError::SendTransactionCallFailed(
                e.to_string(),
            ));
        }
    };

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

fn sol_rpc_client<R: CanisterRuntime>(runtime: &R) -> SolRpcClient<impl Runtime> {
    read_state(|state| state.sol_rpc_client(runtime.inter_canister_call_runtime()))
}

#[derive(Debug, Error)]
enum SubmitTransactionError {
    #[error("failed to create transaction: {0}")]
    CreateTransactionFailed(#[from] CreateTransferError),
    #[error("inconsistent `sendTransaction` results")]
    SendTransactionConsensusError,
    #[error("`sendTransaction` call failed: {0}")]
    SendTransactionCallFailed(String),
}
