use crate::{
    guard::TimerGuard,
    rpc_executor::{MAX_SIGNATURES_PER_STATUS_CHECK, WorkItem, enqueue, execute_rpc_queue},
    runtime::CanisterRuntime,
    state::{TaskType, event::VersionedMessage, read_state},
};
use icrc_ledger_types::icrc1::account::Account;
use itertools::Itertools;
use sol_rpc_types::Slot;
use solana_signature::Signature;
use std::{collections::BTreeMap, time::Duration};

#[cfg(test)]
mod tests;

pub const FINALIZE_TRANSACTIONS_DELAY: Duration = Duration::from_mins(2);
pub const RESUBMIT_TRANSACTIONS_DELAY: Duration = Duration::from_mins(3);

/// Read all submitted transactions from state and enqueue
/// [`WorkItem::CheckSignatureStatuses`] items for the executor, then trigger
/// the executor immediately.
pub async fn finalize_transactions<R: CanisterRuntime>(runtime: R) {
    let _guard = match TimerGuard::new(TaskType::FinalizeTransactions) {
        Ok(guard) => guard,
        Err(_) => return,
    };

    let all_transactions: BTreeMap<Signature, Slot> = read_state(|state| {
        state
            .submitted_transactions()
            .iter()
            .map(|(sig, tx)| (*sig, tx.slot))
            .collect()
    });
    if all_transactions.is_empty() {
        return;
    }

    let signatures: Vec<Signature> = all_transactions.keys().copied().collect();
    for batch in signatures
        .into_iter()
        .chunks(MAX_SIGNATURES_PER_STATUS_CHECK)
        .into_iter()
    {
        let batch: Vec<Signature> = batch.collect();
        let submitted_slots: BTreeMap<Signature, Slot> = batch
            .iter()
            .map(|sig| (*sig, all_transactions[sig]))
            .collect();
        enqueue(WorkItem::CheckSignatureStatuses {
            signatures: batch,
            submitted_slots,
        });
    }

    runtime.set_timer(Duration::ZERO, execute_rpc_queue);
}

/// Read all transactions-to-resubmit from state and enqueue
/// [`WorkItem::ResubmitTransaction`] items for the executor, then trigger the
/// executor immediately.
pub async fn resubmit_transactions<R: CanisterRuntime>(runtime: R) {
    let _guard = match TimerGuard::new(TaskType::ResubmitTransactions) {
        Ok(guard) => guard,
        Err(_) => return,
    };

    let to_resubmit: Vec<(Signature, VersionedMessage, Vec<Account>)> = read_state(|state| {
        state
            .transactions_to_resubmit()
            .iter()
            .map(|(sig, tx)| (*sig, tx.message.clone(), tx.signers.clone()))
            .collect()
    });
    if to_resubmit.is_empty() {
        return;
    }

    for (old_signature, message, signers) in to_resubmit {
        enqueue(WorkItem::ResubmitTransaction {
            old_signature,
            message,
            signers,
        });
    }

    runtime.set_timer(Duration::ZERO, execute_rpc_queue);
}
