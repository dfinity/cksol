use crate::{
    guard::TimerGuard,
    runtime::CanisterRuntime,
    sol_transfer::MAX_TRANSFERS_PER_TX,
    state::{TaskType, audit::process_event, event::EventType, mutate_state, read_state},
};
use std::time::Duration;

pub const DEPOSIT_CONSOLIDATION_DELAY: Duration = Duration::from_mins(10);

pub async fn consolidate_deposits<R: CanisterRuntime>(runtime: R) {
    let _guard = match TimerGuard::new(TaskType::DepositConsolidation) {
        Ok(guard) => guard,
        Err(_) => return,
    };

    while read_state(|state| !state.funds_to_consolidate().is_empty()) {
        let funds_to_consolidate = read_state(|state| {
            state
                .funds_to_consolidate()
                .clone()
                .into_iter()
                .take(MAX_TRANSFERS_PER_TX as usize)
                .collect()
        });
        mutate_state(|state| {
            process_event(
                state,
                EventType::ConsolidatedDeposits {
                    deposits: funds_to_consolidate,
                },
                &runtime,
            )
        });
        // TODO DEFI-2670: Build and submit consolidation transaction
    }
}
