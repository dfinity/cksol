use crate::{
    guard::TimerGuard,
    runtime::CanisterRuntime,
    state::{TaskType, audit::process_event, event::EventType, mutate_state, read_state},
};
use std::time::Duration;

pub const DEPOSIT_CONSOLIDATION_DELAY: Duration = Duration::from_mins(10);
// The maximum number of transfer instructions we can safely fit inside a single Solana transaction.
const MAX_CONSOLIDATIONS_PER_TRANSACTION: usize = 10;

pub async fn consolidate_deposits<R: CanisterRuntime>(runtime: R) {
    let _guard = match TimerGuard::new(TaskType::DepositConsolidation) {
        Ok(guard) => guard,
        Err(_) => return,
    };

    while let Some(funds_to_consolidate) =
        read_state(|state| state.next_funds_to_consolidate(MAX_CONSOLIDATIONS_PER_TRANSACTION))
    {
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
