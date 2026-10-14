use crate::{
    constants::{MAX_ADDRESSES_PER_GET_MULTIPLE_ACCOUNTS, MAX_CONCURRENT_RPC_CALLS},
    deposit::sweep::queue_deposit,
    guard::TimerGuard,
    rpc::{GetMultipleAccountsError, get_balances},
    runtime::CanisterRuntime,
    state::{
        AutomaticDeposit, DepositBalance, QueuedDeposit, State, TaskType, WatchedAddress,
        mutate_state, read_state, watchlist::MAX_WATCHED_ADDRESSES,
    },
    storage::{ScanOutcome, record_deposit_address_scans},
    utils::timed_sized_map::Timestamp,
};
use canlog::log;
use cksol_types::Lamport;
use cksol_types_internal::log::Priority;
use solana_address::Address;

#[cfg(test)]
mod tests;

const _: () = assert!(
    MAX_WATCHED_ADDRESSES.get()
        <= MAX_ADDRESSES_PER_GET_MULTIPLE_ACCOUNTS * MAX_CONCURRENT_RPC_CALLS,
    "BUG: one tick must cover the whole watchlist"
);

/// Reads the balances of the watched deposit addresses that are due for a scan and queues
/// a deposit for every address holding at least the minimum deposit amount, as `deposit_sol`
/// would, with the automatic deposit fee in force.
///
/// A tick that finds the previous one still waiting for its calls returns at once, and the
/// watchlist holds at most [`MAX_CONCURRENT_RPC_CALLS`] calls of
/// [`MAX_ADDRESSES_PER_GET_MULTIPLE_ACCOUNTS`] addresses, so that many calls are in flight
/// at most and one tick covers the whole watchlist. A call that fails counts
/// as a scan of its addresses, which advance along their schedule as after a balance below
/// the minimum, so that the cost of a registration stays within its schedule whatever the
/// SOL RPC canister does.
pub async fn scan_watched_addresses<R: CanisterRuntime>(runtime: R) {
    let _guard = match TimerGuard::new(TaskType::ScanWatchedAddresses) {
        Ok(guard) => guard,
        Err(_) => return,
    };

    let now = Timestamp::from_nanos(runtime.time());
    let due = mutate_state(|state| {
        state.watchlist_mut().evict_expired(now);
        state
            .watchlist()
            .due(now)
            .into_iter()
            .filter(|watched| !state.is_deposit_sol_in_progress(&watched.account))
            .collect::<Vec<_>>()
    });
    if due.is_empty() {
        return;
    }
    let Some(automatic_deposit_fee) = read_state(|state| state.automatic_deposit_fee()) else {
        log!(
            Priority::Error,
            "Skipping the scan of {} watched deposit addresses: the automatic deposit fee is not configured",
            due.len()
        );
        return;
    };

    let chunks: Vec<&[WatchedAddress]> = due
        .chunks(MAX_ADDRESSES_PER_GET_MULTIPLE_ACCOUNTS)
        .collect();
    let reads =
        futures::future::join_all(chunks.iter().map(|chunk| read_balances(&runtime, chunk))).await;

    let scan = Scan {
        scanned_at: Timestamp::from_nanos(runtime.time()),
        automatic_deposit_fee,
    };
    for (chunk, read) in chunks.into_iter().zip(reads) {
        match read {
            Ok(balances) => {
                for (watched, balance) in chunk.iter().zip(balances) {
                    if let Some(outcome) = apply_balance_read(&runtime, scan, watched, balance) {
                        record_deposit_address_scans(outcome, 1);
                    }
                }
            }
            Err(e) => {
                log!(
                    Priority::Info,
                    "Failed to read the balances of {} watched deposit addresses, which advance to their next scan: {e}",
                    chunk.len()
                );
                record_failed_read(scan.scanned_at, chunk);
                record_deposit_address_scans(ScanOutcome::Failed, chunk.len() as u64);
            }
        }
    }
}

async fn read_balances<R: CanisterRuntime>(
    runtime: &R,
    chunk: &[WatchedAddress],
) -> Result<Vec<Lamport>, GetMultipleAccountsError> {
    let addresses: Vec<Address> = chunk.iter().map(|watched| watched.address).collect();
    get_balances(runtime, &addresses).await
}

/// What one tick of the scan applies to every balance it read: the time of the reads and the
/// automatic deposit fee in force, which a deposit detected by the tick records.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Scan {
    scanned_at: Timestamp,
    automatic_deposit_fee: Lamport,
}

/// Applies the balance read for a watched address: queues a deposit if the balance reaches
/// the minimum deposit amount, advances the scan schedule otherwise, and does nothing when
/// the registration the read was made for is gone, replaced or expired, or when a deposit
/// of the account is in flight, since the read says nothing about the current one then.
fn apply_balance_read<R: CanisterRuntime>(
    runtime: &R,
    scan: Scan,
    watched: &WatchedAddress,
    balance: Lamport,
) -> Option<ScanOutcome> {
    let Scan {
        scanned_at,
        automatic_deposit_fee,
    } = scan;
    mutate_state(|state| {
        if !is_same_registration(state, scanned_at, watched)
            || state.deposits().in_flight_id(&watched.account).is_some()
        {
            return None;
        }
        if balance < state.minimum_deposit_amount() {
            state
                .watchlist_mut()
                .record_scan(scanned_at, &watched.account);
            return Some(ScanOutcome::BelowMinimum);
        }
        let balance = DepositBalance::new(balance)
            .expect("BUG: the minimum deposit amount covers the rent exemption threshold");
        queue_deposit(
            runtime,
            state,
            QueuedDeposit {
                account: watched.account,
                address: watched.address,
                balance,
                automatic: Some(AutomaticDeposit {
                    fee: automatic_deposit_fee,
                    valid_until: watched.valid_until.as_nanos(),
                }),
            },
        );
        Some(ScanOutcome::Queued)
    })
}

fn record_failed_read(scanned_at: Timestamp, chunk: &[WatchedAddress]) {
    mutate_state(|state| {
        for watched in chunk {
            if is_same_registration(state, scanned_at, watched) {
                state
                    .watchlist_mut()
                    .record_scan(scanned_at, &watched.account);
            }
        }
    });
}

fn is_same_registration(state: &State, now: Timestamp, watched: &WatchedAddress) -> bool {
    state
        .watchlist()
        .get(now, &watched.account)
        .is_some_and(|current| current.valid_until == watched.valid_until)
}
