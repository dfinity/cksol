use crate::{
    address::{account_address, lazy_get_schnorr_master_key},
    constants::MAX_CONCURRENT_RPC_CALLS,
    guard::TimerGuard,
    rpc::get_signatures_for_address,
    runtime::CanisterRuntime,
    state::{
        SchnorrPublicKey, TaskType, audit::process_event, event::EventType, mutate_state,
        read_state,
    },
    storage::{with_automatic_deposit_cache, with_automatic_deposit_cache_mut},
};
use cache::{
    AccountMonitoringState, AutomaticDepositCacheEntry, AutomaticDepositCacheExt,
    INITIAL_BACKOFF_DELAY_MINS,
};
use canlog::log;
use cksol_types::UpdateBalanceError;
use cksol_types_internal::log::Priority;
use icrc_ledger_types::icrc1::account::Account;
use sol_rpc_types::{CommitmentLevel, GetSignaturesForAddressParams};
use std::time::Duration;

pub(crate) mod cache;

#[cfg(test)]
mod tests;

/// Maximum number of accounts the minter will monitor simultaneously for automated deposits.
pub const MAX_MONITORED_ACCOUNTS: usize = 100;

/// How often the minter polls monitored addresses for new deposit transactions.
pub const POLL_MONITORED_ADDRESSES_DELAY: Duration = Duration::from_mins(1);

/// Maximum number of signatures to fetch per `getSignaturesForAddress` call.
pub const GET_SIGNATURES_FOR_ADDRESS_LIMIT: u32 = 100;

/// Registers the given account for automated deposit monitoring.
///
/// - If the account is already actively monitored (has a scheduled poll), returns `Ok(())`.
/// - If the account's RPC quota is exhausted, returns `Err(MonitoringQuotaExhausted)`.
/// - If the account has remaining quota but is not scheduled (e.g. was stopped), reschedules it.
/// - If the account is unknown, allocates a fresh quota and schedules the first poll.
/// - Returns `Err(QueueFull)` if the monitored account limit has been reached.
pub fn update_balance<R: CanisterRuntime>(
    runtime: &R,
    account: Account,
) -> Result<(), UpdateBalanceError> {
    let cached_entry = match with_automatic_deposit_cache(|cache| cache.monitoring_state(&account))
    {
        AccountMonitoringState::Active { .. } => {
            debug_assert!(read_state(|s| s.monitored_accounts().contains(&account)));
            return Ok(());
        }
        AccountMonitoringState::Exhausted { .. } => {
            return Err(UpdateBalanceError::MonitoringQuotaExhausted);
        }
        AccountMonitoringState::Stopped { entry } => Some(entry),
        AccountMonitoringState::Unknown => None,
    };

    if read_state(|state| state.monitored_accounts().len()) >= MAX_MONITORED_ACCOUNTS {
        return Err(UpdateBalanceError::QueueFull);
    }

    debug_assert!(!read_state(|s| s.monitored_accounts().contains(&account)));
    mutate_state(|state| {
        process_event(
            state,
            EventType::StartedMonitoringAccount { account },
            runtime,
        );
    });

    let new_entry = match cached_entry {
        Some(entry) => AutomaticDepositCacheEntry {
            rpc_quota_left: entry.rpc_quota_left,
            next_backoff_delay_mins: INITIAL_BACKOFF_DELAY_MINS,
        },
        None => AutomaticDepositCacheEntry::default(),
    };
    let next_poll_at = runtime.time() + POLL_MONITORED_ADDRESSES_DELAY.as_nanos() as u64;
    with_automatic_deposit_cache_mut(|cache| {
        cache.insert(account, next_poll_at, new_entry);
    });

    Ok(())
}

/// Polls all monitored addresses that are due for a check.
///
/// For each due address, calls `getSignaturesForAddress` on the Solana RPC.
/// After each call, reschedules the account with exponential backoff, or marks it
/// as stopped (index `u64::MAX`) if the RPC quota has been exhausted.
pub async fn poll_monitored_addresses<R: CanisterRuntime>(runtime: R) {
    let _guard = match TimerGuard::new(TaskType::PollMonitoredAddresses) {
        Ok(guard) => guard,
        Err(_) => return,
    };

    let now = runtime.time();

    let due: Vec<(Account, AutomaticDepositCacheEntry)> = with_automatic_deposit_cache(|cache| {
        cache
            .iter()
            .take_while(|(t, ..)| *t <= now)
            .map(|(_, account, entry)| (account, entry))
            // +1 to detect whether more accounts remain after this round.
            .take(MAX_CONCURRENT_RPC_CALLS + 1)
            .collect()
    });

    if due.is_empty() {
        return;
    }

    let more_to_process = due.len() > MAX_CONCURRENT_RPC_CALLS;
    let reschedule = scopeguard::guard(runtime.clone(), |runtime| {
        runtime.set_timer(Duration::ZERO, poll_monitored_addresses);
    });

    let master_key = lazy_get_schnorr_master_key(&runtime).await;

    futures::future::join_all(
        due.into_iter()
            .take(MAX_CONCURRENT_RPC_CALLS)
            .map(|(account, entry)| poll_account(&runtime, &master_key, account, entry)),
    )
    .await;

    if !more_to_process {
        scopeguard::ScopeGuard::into_inner(reschedule);
    }
}

async fn poll_account<R: CanisterRuntime>(
    runtime: &R,
    master_key: &SchnorrPublicKey,
    account: Account,
    entry: AutomaticDepositCacheEntry,
) {
    let deposit_address = account_address(master_key, &account);

    let params = GetSignaturesForAddressParams {
        pubkey: deposit_address.into(),
        commitment: Some(CommitmentLevel::Finalized),
        limit: Some(
            GET_SIGNATURES_FOR_ADDRESS_LIMIT
                .try_into()
                .expect("GET_SIGNATURES_FOR_ADDRESS_LIMIT must be between 1 and 1000"),
        ),
        min_context_slot: None,
        before: None,
        until: None,
    };

    let rpc_quota_left = entry.rpc_quota_left.saturating_sub(1);

    match get_signatures_for_address(runtime, params).await {
        Err(e) => {
            log!(
                Priority::Info,
                "Failed to get signatures for address {deposit_address}: {e}"
            );
        }
        Ok(_signatures) => {
            // TODO(DEFI-2780): Process discovered deposit signatures.
        }
    }

    if rpc_quota_left == 0 {
        // Use u64::MAX so the entry is retained but never scheduled for another poll.
        with_automatic_deposit_cache_mut(|cache| {
            cache.insert(
                account,
                u64::MAX,
                AutomaticDepositCacheEntry {
                    rpc_quota_left,
                    next_backoff_delay_mins: entry.next_backoff_delay_mins,
                },
            );
        });
        debug_assert!(read_state(|s| s.monitored_accounts().contains(&account)));
        mutate_state(|state| {
            process_event(
                state,
                EventType::StoppedMonitoringAccount { account },
                runtime,
            );
        });
        log!(
            Priority::Info,
            "Stopped monitoring {deposit_address}: RPC quota exhausted"
        );
    } else {
        let delay_mins = entry.next_backoff_delay_mins;
        let delay = Duration::from_mins(delay_mins);
        let delay_ns: u64 = delay.as_nanos().try_into().unwrap_or(u64::MAX - 1);
        let next_poll_at = runtime.time().saturating_add(delay_ns).min(u64::MAX - 1);
        with_automatic_deposit_cache_mut(|cache| {
            cache.insert(
                account,
                next_poll_at,
                AutomaticDepositCacheEntry {
                    rpc_quota_left,
                    next_backoff_delay_mins: delay_mins.saturating_mul(2),
                },
            );
        });
    }
}
