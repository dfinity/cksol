use crate::{
    address::{account_address, lazy_get_schnorr_master_key},
    constants::MAX_CONCURRENT_RPC_CALLS,
    deposit::fetch_and_validate_deposit,
    guard::TimerGuard,
    ledger::mint,
    rpc::get_signatures_for_address,
    runtime::CanisterRuntime,
    state::{
        SchnorrPublicKey, TaskType,
        audit::process_event,
        event::{DepositId, DepositSource, EventType},
        mutate_state, read_state,
    },
};
use canlog::log;
use cksol_types::UpdateBalanceError;
use cksol_types_internal::log::Priority;
use icrc_ledger_types::icrc1::account::Account;
use sol_rpc_types::{CommitmentLevel, GetSignaturesForAddressParams, Lamport};
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

/// How often the minter processes the pending-signatures queue.
pub const PROCESS_PENDING_SIGNATURES_DELAY: Duration = Duration::from_secs(5);

/// How often the minter attempts to mint accepted automatic deposits.
pub const MINT_AUTOMATIC_DEPOSITS_DELAY: Duration = Duration::from_secs(5);

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

/// Processes pending deposit signatures using a round-robin, capacity-filling strategy.
///
/// Each pass takes one signature per account (fair round-robin by `Account` key order). If
/// capacity remains after a full pass, another pass begins — so up to `MAX_CONCURRENT_RPC_CALLS`
/// signatures are dispatched in parallel each call. For each signature, calls `getTransaction`
/// and emits [`EventType::AcceptedDeposit`] with `source: Automatic` if valid. Invalid or
/// already-processed signatures are silently discarded. Reschedules itself immediately if
/// signatures remain after the capacity is exhausted.
pub async fn process_pending_signatures<R: CanisterRuntime>(runtime: R) {
    let _guard = match TimerGuard::new(TaskType::ProcessPendingSignatures) {
        Ok(guard) => guard,
        Err(_) => return,
    };

    // Round-robin across accounts, refilling capacity with additional passes until exhausted.
    let to_process: Vec<(Account, Signature)> = PENDING_SIGNATURES.with(|pending| {
        let mut pending = pending.borrow_mut();
        let mut to_process = Vec::with_capacity(MAX_CONCURRENT_RPC_CALLS);
        let mut capacity = MAX_CONCURRENT_RPC_CALLS;
        loop {
            let before = to_process.len();
            for (account, queue) in pending.iter_mut() {
                if capacity == 0 {
                    break;
                }
                if let Some(sig) = queue.pop_front() {
                    to_process.push((*account, sig));
                    capacity -= 1;
                }
            }
            if to_process.len() == before || capacity == 0 {
                break;
            }
        }
        pending.retain(|_, queue| !queue.is_empty());
        to_process
    });

    if to_process.is_empty() {
        return;
    }

    let more_to_process = PENDING_SIGNATURES.with(|p| !p.borrow().is_empty());
    let reschedule = scopeguard::guard(runtime.clone(), |runtime| {
        runtime.set_timer(Duration::ZERO, process_pending_signatures);
    });

    let fee = read_state(|s| s.automated_deposit_fee());

    futures::future::join_all(
        to_process
            .into_iter()
            .map(|(account, signature)| process_signature(&runtime, account, signature, fee)),
    )
    .await;

    if !more_to_process {
        scopeguard::ScopeGuard::into_inner(reschedule);
    }
}

async fn process_signature<R: CanisterRuntime>(
    runtime: &R,
    account: Account,
    signature: Signature,
    fee: u64,
) {
    // Skip signatures that were already accepted or minted (e.g. via manual deposit).
    let deposit_id = DepositId { account, signature };
    if read_state(|s| s.deposit_status(&deposit_id)).is_some() {
        return;
    }

    match fetch_and_validate_deposit(runtime, account, signature, fee).await {
        Ok((deposit_id, deposit_amount, amount_to_mint)) => {
            mutate_state(|state| {
                process_event(
                    state,
                    EventType::AcceptedDeposit {
                        deposit_id,
                        deposit_amount,
                        amount_to_mint,
                        source: DepositSource::Automatic,
                    },
                    runtime,
                )
            });
            log!(
                Priority::Info,
                "Accepted automatic deposit {deposit_id:?}: {deposit_amount} lamports deposited, minting {amount_to_mint} lamports"
            );
        }
        Err(e) => {
            log!(
                Priority::Info,
                "Discarding automatic deposit signature {signature}: {e}"
            );
        }
    }
}

/// Drains accepted automatic deposits and mints ckSOL for each.
///
/// Processes up to [`MAX_CONCURRENT_RPC_CALLS`] deposits per round and
/// reschedules itself at `Duration::ZERO` if more remain.
pub async fn mint_automatic_deposits<R: CanisterRuntime>(runtime: R) {
    let _guard = match TimerGuard::new(TaskType::Mint) {
        Ok(guard) => guard,
        Err(_) => return,
    };

    let to_mint: Vec<(DepositId, Lamport)> = read_state(|s| {
        s.accepted_deposits()
            .iter()
            .filter(|(_, d)| d.source == DepositSource::Automatic)
            .take(MAX_CONCURRENT_RPC_CALLS)
            .map(|(deposit_id, deposit)| (*deposit_id, deposit.amount_to_mint))
            .collect()
    });

    if to_mint.is_empty() {
        return;
    }

    let more_to_process = read_state(|s| {
        s.accepted_deposits()
            .iter()
            .filter(|(_, d)| d.source == DepositSource::Automatic)
            .count()
            > MAX_CONCURRENT_RPC_CALLS
    });
    let reschedule = scopeguard::guard(runtime.clone(), |runtime| {
        runtime.set_timer(Duration::ZERO, mint_automatic_deposits);
    });

    futures::future::join_all(
        to_mint
            .into_iter()
            .map(|(deposit_id, amount_to_mint)| mint_one(&runtime, deposit_id, amount_to_mint)),
    )
    .await;

    if !more_to_process {
        scopeguard::ScopeGuard::into_inner(reschedule);
    }
}

async fn mint_one<R: CanisterRuntime>(runtime: &R, deposit_id: DepositId, amount_to_mint: Lamport) {
    match mint(runtime, deposit_id, amount_to_mint).await {
        Ok(_) => {}
        Err(e) => {
            log!(
                Priority::Info,
                "Failed to mint ckSOL for automatic deposit {deposit_id:?}: {e}. Will retry."
            );
        }
    }
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
