use crate::{
    guard::TimerGuard,
    numeric::LedgerMintIndex,
    rpc_executor::{MAX_TRANSFERS_PER_CONSOLIDATION, WorkItem, enqueue, execute_rpc_queue},
    runtime::CanisterRuntime,
    state::{TaskType, read_state},
};
use icrc_ledger_types::icrc1::account::Account;
use itertools::Itertools;
use sol_rpc_types::Lamport;
use std::{collections::BTreeMap, time::Duration};

#[cfg(test)]
mod tests;

pub const DEPOSIT_CONSOLIDATION_DELAY: Duration = Duration::from_mins(10);

pub async fn consolidate_deposits<R: CanisterRuntime>(runtime: R) {
    let _guard = match TimerGuard::new(TaskType::DepositConsolidation) {
        Ok(guard) => guard,
        Err(_) => return,
    };

    let all_deposits = read_state(|s| group_deposits_by_account(s.deposits_to_consolidate()));
    if all_deposits.is_empty() {
        return;
    }

    for batch in all_deposits
        .into_iter()
        .chunks(MAX_TRANSFERS_PER_CONSOLIDATION)
        .into_iter()
    {
        enqueue(WorkItem::SubmitConsolidationBatch(batch.collect()));
    }

    runtime.set_timer(Duration::ZERO, execute_rpc_queue);
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
