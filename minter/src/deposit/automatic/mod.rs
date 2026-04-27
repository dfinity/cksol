use crate::{
    address::{account_address, lazy_get_schnorr_master_key},
    constants::MAX_CONCURRENT_RPC_CALLS,
    guard::{TimerGuard, too_many_http_outcalls},
    rpc::get_signatures_for_address,
    runtime::CanisterRuntime,
    state::{
        SchnorrPublicKey, TaskType, audit::process_event, event::EventType, mutate_state,
        read_state,
    },
};
use canlog::log;
use cksol_types::UpdateBalanceError;
use cksol_types_internal::log::Priority;
use icrc_ledger_types::icrc1::account::Account;
use sol_rpc_types::{CommitmentLevel, GetSignaturesForAddressParams};
use solana_signature::Signature;
use std::{
    cell::RefCell,
    collections::{BTreeMap, VecDeque},
    time::Duration,
};

thread_local! {
    static PENDING_SIGNATURES: RefCell<BTreeMap<Account, VecDeque<Signature>>> =
        RefCell::default();
}

#[cfg(test)]
mod tests;

/// Maximum number of accounts the minter will monitor simultaneously for automated deposits.
pub const MAX_MONITORED_ACCOUNTS: usize = 100;

/// How often the minter polls monitored addresses for new deposit transactions.
pub const POLL_MONITORED_ADDRESSES_DELAY: Duration = Duration::from_mins(1);

/// Maximum number of `getTransaction` calls to make per polled account.
pub const MAX_TRANSACTIONS_PER_ACCOUNT: usize = 10;

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

/// Polls all monitored addresses for new deposit transaction signatures.
///
/// For each address, calls `getSignaturesForAddress` on the Solana RPC.
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

    let more_to_process = all_accounts.len() > MAX_CONCURRENT_RPC_CALLS;
    let reschedule = scopeguard::guard(runtime.clone(), |runtime| {
        runtime.set_timer(Duration::ZERO, poll_monitored_addresses);
    });

    if too_many_http_outcalls() {
        log!(
            Priority::Info,
            "Too many concurrent HTTP outcalls, rescheduling poll_monitored_addresses"
        );
        return;
    }

    let master_key = lazy_get_schnorr_master_key(&runtime).await;

    futures::future::join_all(
        all_accounts
            .into_iter()
            .take(MAX_CONCURRENT_RPC_CALLS)
            .map(|account| poll_account(&runtime, &master_key, account)),
    )
    .await;

    if !more_to_process {
        // All work fits in this round
        scopeguard::ScopeGuard::into_inner(reschedule);
    }
}

async fn poll_account<R: CanisterRuntime>(
    runtime: &R,
    master_key: &SchnorrPublicKey,
    account: Account,
) {
    let deposit_address = account_address(master_key, &account);

    let params = GetSignaturesForAddressParams {
        pubkey: deposit_address.into(),
        commitment: Some(CommitmentLevel::Finalized),
        min_context_slot: None,
        // Fetch no more signatures than we intend to process with `getTransaction`.
        limit: Some(
            (MAX_TRANSACTIONS_PER_ACCOUNT as u32)
                .try_into()
                .expect("MAX_TRANSACTIONS_PER_ACCOUNT must be between 1 and 1000"),
        ),
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
        Ok(signatures) => {
            let new_sigs: Vec<Signature> = signatures
                .into_iter()
                .filter(|s| s.err.is_none())
                .map(|s| s.signature.into())
                .collect();
            if !new_sigs.is_empty() {
                PENDING_SIGNATURES.with(|pending| {
                    pending
                        .borrow_mut()
                        .entry(account)
                        .or_default()
                        .extend(new_sigs);
                });
            }
        }
    }

    mutate_state(|state| {
        process_event(
            state,
            EventType::StoppedMonitoringAccount { account },
            runtime,
        );
    });
}

#[cfg(any(test, feature = "canbench-rs"))]
pub fn pending_signatures_for(account: &Account) -> Vec<Signature> {
    PENDING_SIGNATURES.with(|p| {
        p.borrow()
            .get(account)
            .map(|q| q.iter().copied().collect())
            .unwrap_or_default()
    })
}

#[cfg(any(test, feature = "canbench-rs"))]
pub fn reset_pending_signatures() {
    PENDING_SIGNATURES.with(|p| p.borrow_mut().clear());
}
