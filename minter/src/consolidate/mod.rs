use crate::{
    constants::MAX_CONCURRENT_RPC_CALLS,
    guard::TimerGuard,
    numeric::LedgerMintIndex,
    rpc::{SubmitTransactionError, get_recent_slot_and_blockhash, submit_transaction},
    runtime::CanisterRuntime,
    sol_transfer::{CreateTransferError, MAX_SIGNATURES, create_signed_consolidation_transaction},
    state::{
        TaskType,
        audit::process_event,
        event::{EventType, TransactionPurpose},
        mutate_state, read_state,
    },
};
use canlog::log;
use cksol_types_internal::log::Priority;
use icrc_ledger_types::icrc1::account::Account;
use itertools::Itertools;
use sol_rpc_types::{Lamport, Slot};
use solana_hash::Hash;
use solana_signature::Signature;
use std::collections::BTreeMap;
use std::time::Duration;
use thiserror::Error;

#[cfg(test)]
mod tests;

pub const DEPOSIT_CONSOLIDATION_DELAY: Duration = Duration::from_mins(10);

pub(crate) const MAX_TRANSFERS_PER_CONSOLIDATION: usize = MAX_SIGNATURES as usize;

pub async fn consolidate_deposits<R: CanisterRuntime>(runtime: R) {
    let _guard = match TimerGuard::new(TaskType::DepositConsolidation) {
        Ok(guard) => guard,
        Err(_) => return,
    };

    let all_deposits = read_state(|s| group_deposits_by_account(s.deposits_to_consolidate()));
    let more_to_process =
        all_deposits.len() > MAX_CONCURRENT_RPC_CALLS * MAX_TRANSFERS_PER_CONSOLIDATION;
    let reschedule = scopeguard::guard(runtime.clone(), |runtime| {
        runtime.set_timer(Duration::ZERO, consolidate_deposits);
    });

    let batches: Vec<Vec<_>> = all_deposits
        .into_iter()
        .chunks(MAX_TRANSFERS_PER_CONSOLIDATION)
        .into_iter()
        .take(MAX_CONCURRENT_RPC_CALLS)
        .map(Iterator::collect)
        .collect();

    if batches.is_empty() {
        // Nothing to process
        scopeguard::ScopeGuard::into_inner(reschedule);
        return;
    }

    let (slot, recent_blockhash) = match get_recent_slot_and_blockhash(&runtime).await {
        Ok(result) => result,
        Err(e) => {
            log!(Priority::Info, "Failed to fetch recent blockhash: {e}");
            return;
        }
    };

    futures::future::join_all(batches.into_iter().map(async |funds| {
        match submit_consolidation_transaction(&runtime, funds, slot, recent_blockhash).await {
            Ok(sig) => log!(Priority::Info, "Submitted consolidation transaction {sig}"),
            Err(e) => log!(Priority::Info, "Deposit consolidation failed: {e}"),
        }
    }))
    .await;

    if !more_to_process {
        // All work fits in this round
        scopeguard::ScopeGuard::into_inner(reschedule);
    }
}

fn group_deposits_by_account(
    deposits: &BTreeMap<LedgerMintIndex, (Account, Lamport)>,
) -> Vec<(Account, (Lamport, Vec<LedgerMintIndex>))> {
    let mut by_account: BTreeMap<Account, (Lamport, Vec<LedgerMintIndex>)> = BTreeMap::new();
    for (mint_index, (account, lamport)) in deposits {
        let entry = by_account.entry(*account).or_default();
        entry.0 += lamport;
        entry.1.push(*mint_index);
    }
    by_account.into_iter().collect()
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
    funds_to_consolidate: Vec<(Account, (Lamport, Vec<LedgerMintIndex>))>,
    slot: Slot,
    recent_blockhash: Hash,
) -> Result<Signature, ConsolidationError> {
    let (sources, mint_indices): (Vec<_>, Vec<_>) = funds_to_consolidate
        .into_iter()
        .map(|(account, (lamport, indices))| ((account, lamport), indices))
        .unzip();
    let (transaction, signers) =
        create_signed_consolidation_transaction(runtime, sources, recent_blockhash).await?;

    let signature = transaction.signatures[0];
    let message = transaction.message.clone();

    mutate_state(|state| {
        process_event(
            state,
            EventType::SubmittedTransaction {
                signature,
                message: message.into(),
                signers,
                slot,
                purpose: TransactionPurpose::ConsolidateDeposits {
                    mint_indices: mint_indices.into_iter().flatten().collect(),
                },
            },
            runtime,
        )
    });

    submit_transaction(runtime, transaction).await?;

    Ok(signature)
}
