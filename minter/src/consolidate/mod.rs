use crate::{
    guard::TimerGuard,
    runtime::CanisterRuntime,
    state::{TaskType, audit::process_event, event::EventType, mutate_state, read_state},
};
use std::time::Duration;

const DEPOSIT_CONSOLIDATION_DELAY: Duration = Duration::from_mins(10);
// The maximum number of transfer instructions we can safely fit inside a single Solana transaction.
const MAX_CONSOLIDATIONS_PER_TRANSACTION: usize = 10;

// TODO DEFI-2670: Consider smarter scheduling of the consolidation task, e.g. making sure only
//  scheduling the task if it is not already scheduled, or only if there are a certain number of
//  non-consolidated deposits.
pub fn schedule_deposit_consolidation<R: CanisterRuntime>(runtime: R) {
    runtime.set_timer(
        DEPOSIT_CONSOLIDATION_DELAY,
        consolidate_deposits(runtime.clone()),
    );
}

async fn consolidate_deposits<R: CanisterRuntime>(runtime: R) {
    let reschedule_task_guard = scopeguard::guard(runtime.clone(), |runtime| {
        schedule_deposit_consolidation(runtime)
    });

    let _guard = match TimerGuard::new(TaskType::DepositConsolidation) {
        Ok(guard) => guard,
        Err(_) => return,
    };

    let funds_to_consolidate: Vec<_> = read_state(|state| {
        state
            .funds_to_consolidate()
            .iter()
            .take(MAX_CONSOLIDATIONS_PER_TRANSACTION)
            .map(|(account, amount)| (*account, *amount))
            .collect()
    });
    // Note that this should not happen since the task should not have been scheduled in this case.
    if funds_to_consolidate.is_empty() {
        return;
    }

    // TODO DEFI-2670: Build and submit consolidation transaction

    mutate_state(|state| {
        process_event(
            state,
            EventType::ConsolidatedDeposits {
                deposits: funds_to_consolidate,
            },
            &runtime,
        )
    });

    if read_state(|state| state.funds_to_consolidate().is_empty()) {
        // No more deposits to consolidate, defuse guard
        scopeguard::ScopeGuard::into_inner(reschedule_task_guard);
    }
}
