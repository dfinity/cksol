use crate::{
    guard::TimerGuard,
    rpc_executor::{WorkItem, enqueue, execute_rpc_queue},
    runtime::CanisterRuntime,
    state::{TaskType, audit::process_event, event::EventType, mutate_state, read_state},
};
use canlog::log;
use cksol_types::UpdateBalanceError;
use cksol_types_internal::log::Priority;
use icrc_ledger_types::icrc1::account::Account;
use std::time::Duration;

#[cfg(test)]
mod tests;

/// Maximum number of accounts the minter will monitor simultaneously for automated deposits.
pub const MAX_MONITORED_ACCOUNTS: usize = 100;

/// How often the minter polls monitored addresses for new deposit transactions.
pub const POLL_MONITORED_ADDRESSES_DELAY: Duration = Duration::from_mins(1);

// Re-export test helpers whose backing storage lives in rpc_executor.
#[cfg(any(test, feature = "canbench-rs"))]
pub use crate::rpc_executor::{pending_signatures_for, reset_pending_signatures};

/// Registers the given account for automated deposit monitoring.
///
/// Returns `Ok(())` if the account was registered (or was already being monitored).
/// Returns `Err(UpdateBalanceError::QueueFull)` if the monitored account queue is at capacity.
pub fn update_balance<R: CanisterRuntime>(
    runtime: &R,
    account: Account,
) -> Result<(), UpdateBalanceError> {
    if read_state(|state| state.monitored_accounts().contains(&account)) {
        return Ok(());
    }

    if read_state(|state| state.monitored_accounts().len() >= MAX_MONITORED_ACCOUNTS) {
        return Err(UpdateBalanceError::QueueFull);
    }

    mutate_state(|state| {
        process_event(
            state,
            EventType::StartedMonitoringAccount { account },
            runtime,
        );
    });
    log!(
        Priority::Info,
        "Started monitoring account {account:?} for automated deposits"
    );

    Ok(())
}

/// Enqueues a [`WorkItem::PollMonitoredAddress`] for every monitored account,
/// then triggers the executor immediately.
pub async fn poll_monitored_addresses<R: CanisterRuntime>(runtime: R) {
    let _guard = match TimerGuard::new(TaskType::PollMonitoredAddresses) {
        Ok(guard) => guard,
        Err(_) => return,
    };

    let all_accounts: Vec<Account> =
        read_state(|s| s.monitored_accounts().iter().copied().collect());
    if all_accounts.is_empty() {
        return;
    }

    for account in all_accounts {
        enqueue(WorkItem::PollMonitoredAddress(account));
    }

    runtime.set_timer(Duration::ZERO, execute_rpc_queue);
}
