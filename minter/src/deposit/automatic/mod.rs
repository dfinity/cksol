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
use cache::AutomaticDepositCacheEntry;
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

/// Maximum number of `getTransaction` calls to make per polled account.
pub const MAX_GET_TRANSACTION_CALLS: usize = 5;

/// Maximum number of `getSignaturesForAddress` calls to make per monitored account before stopping.
/// The delays follow an exponential backoff: 1, 2, 4, ..., 512 minutes (1023 minutes total).
pub const MAX_GET_SIGNATURES_FOR_ADDRESS_CALLS: u8 = 10;

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

    // Schedule first poll in 2^0 = 1 minute.
    let next_poll_at = runtime.time() + Duration::from_mins(1).as_nanos() as u64;
    with_automatic_deposit_cache_mut(|cache| {
        cache.insert(account, next_poll_at, AutomaticDepositCacheEntry::default());
    });

    Ok(())
}

/// Polls all monitored addresses that are due for a check.
///
/// For each due address, calls `getSignaturesForAddress` on the Solana RPC.
/// After each call, reschedules the account with exponential backoff, or emits
/// `StoppedMonitoringAccount` if `MAX_GET_SIGNATURES_FOR_ADDRESS_CALLS` has been reached.
pub async fn poll_monitored_addresses<R: CanisterRuntime>(runtime: R) {
    let _guard = match TimerGuard::new(TaskType::PollMonitoredAddresses) {
        Ok(guard) => guard,
        Err(_) => return,
    };

    let now = runtime.time();

    let due: Vec<(Account, u8)> = with_automatic_deposit_cache(|cache| {
        cache
            .iter()
            .take_while(|(t, ..)| *t <= now)
            .map(|(_, account, entry)| (account, entry.get_signatures_calls))
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

    futures::future::join_all(due.into_iter().take(MAX_CONCURRENT_RPC_CALLS).map(
        |(account, get_signatures_calls)| {
            poll_account(&runtime, &master_key, account, get_signatures_calls)
        },
    ))
    .await;

    if !more_to_process {
        scopeguard::ScopeGuard::into_inner(reschedule);
    }
}

async fn poll_account<R: CanisterRuntime>(
    runtime: &R,
    master_key: &SchnorrPublicKey,
    account: Account,
    get_signatures_calls: u8,
) {
    let deposit_address = account_address(master_key, &account);

    let params = GetSignaturesForAddressParams {
        pubkey: deposit_address.into(),
        commitment: Some(CommitmentLevel::Finalized),
        // Fetch no more signatures than we intend to process with `getTransaction`.
        limit: Some(
            (MAX_GET_TRANSACTION_CALLS as u32)
                .try_into()
                .expect("MAX_GET_TRANSACTION_CALLS must be between 1 and 1000"),
        ),
        min_context_slot: None,
        before: None,
        until: None,
    };

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

    let get_signatures_calls = get_signatures_calls + 1;
    if get_signatures_calls >= MAX_GET_SIGNATURES_FOR_ADDRESS_CALLS {
        // Use u64::MAX so the entry is retained but never scheduled for another poll.
        with_automatic_deposit_cache_mut(|cache| {
            cache.insert(
                account,
                u64::MAX,
                AutomaticDepositCacheEntry {
                    get_signatures_calls,
                },
            );
        });
        mutate_state(|state| {
            process_event(
                state,
                EventType::StoppedMonitoringAccount { account },
                runtime,
            );
        });
        log!(
            Priority::Info,
            "Stopped monitoring {deposit_address}: reached maximum getSignaturesForAddress calls ({MAX_GET_SIGNATURES_FOR_ADDRESS_CALLS})"
        );
    } else {
        // Exponential backoff: delay before next poll is 2^get_signatures_calls minutes.
        let delay = Duration::from_mins(1u64 << get_signatures_calls);
        let next_poll_at = runtime.time() + delay.as_nanos() as u64;
        with_automatic_deposit_cache_mut(|cache| {
            cache.insert(
                account,
                next_poll_at,
                AutomaticDepositCacheEntry {
                    get_signatures_calls,
                },
            );
        });
    }
}
