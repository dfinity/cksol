use crate::{
    runtime::CanisterRuntime,
    state::{audit::process_event, event::EventType, mutate_state, read_state},
};
use cksol_types::UpdateBalanceError;
use icrc_ledger_types::icrc1::account::Account;

#[cfg(test)]
mod tests;

/// Maximum number of accounts the minter will monitor simultaneously for automated deposits.
pub const MAX_MONITORED_ACCOUNTS: usize = 100;

/// Registers the caller's account for automated deposit monitoring.
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

    Ok(())
}
