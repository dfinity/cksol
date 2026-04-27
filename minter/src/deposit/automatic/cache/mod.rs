use crate::utils::sorted_key_map::SortedKeyMap;
use icrc_ledger_types::icrc1::account::Account;

/// Maximum number of `getSignaturesForAddress` calls allowed per monitored account.
pub const MAX_GET_SIGNATURES_CALLS: u32 = 10;

/// Maximum number of `getTransaction` calls allowed per monitored account.
pub const MAX_RETRIEVED_TRANSACTIONS: u32 = 50;

/// Initial backoff delay in minutes before the first poll.
pub const INITIAL_BACKOFF_DELAY_MINS: u64 = 1;

/// Per-account state for automated deposit discovery.
///
/// This cache is intentionally separate from the event log: it can be fully
/// reconstructed by redoing the RPC calls, so there is no need to replay events
/// to restore it. It lives in unstable heap memory and is reset on canister upgrade.
#[derive(Clone, Debug, PartialEq)]
pub struct AutomaticDepositCacheEntry {
    /// Remaining quota for `getSignaturesForAddress` calls.
    pub sig_calls_remaining: u32,
    /// Remaining quota for `getTransaction` calls.
    pub tx_calls_remaining: u32,
    /// The delay in minutes before the next poll. Doubles after each poll.
    pub next_backoff_delay_mins: u64,
}

impl Default for AutomaticDepositCacheEntry {
    fn default() -> Self {
        Self {
            sig_calls_remaining: MAX_GET_SIGNATURES_CALLS,
            tx_calls_remaining: MAX_RETRIEVED_TRANSACTIONS,
            next_backoff_delay_mins: INITIAL_BACKOFF_DELAY_MINS,
        }
    }
}

/// Heap-memory cache storing per-account automated deposit discovery state,
/// ordered by next poll time for efficient scheduling.
///
/// Backed by a [`SortedKeyMap`] with `Account` as key and `u64` (nanosecond timestamp)
/// as the sort index.
///
/// Accounts that have been stopped are stored with `next_poll_at = u64::MAX`
/// so they are never picked up by the poll loop, but their quota is retained
/// for future `update_balance` calls.
#[derive(Default)]
pub struct AutomaticDepositCache(SortedKeyMap<Account, u64, AutomaticDepositCacheEntry>);

impl AutomaticDepositCache {
    /// Returns the current poll time and entry for the given account.
    pub fn get_with_index(&self, account: &Account) -> Option<(u64, AutomaticDepositCacheEntry)> {
        self.0.get_with_index(account).map(|(t, e)| (*t, e.clone()))
    }

    /// Inserts or updates an entry, updating the poll-time index atomically.
    pub fn insert(
        &mut self,
        account: Account,
        next_poll_at: u64,
        entry: AutomaticDepositCacheEntry,
    ) {
        self.0.insert(account, next_poll_at, entry);
    }

    /// Iterates all `(next_poll_at, account, entry)` triples in ascending poll-time order.
    pub fn iter(&self) -> impl Iterator<Item = (u64, Account, AutomaticDepositCacheEntry)> + '_ {
        self.0
            .iter()
            .map(|(t, account, entry)| (*t, *account, entry.clone()))
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// The monitoring lifecycle state of an account, as derived from the cache.
pub enum AccountMonitoringState {
    /// No monitoring information has been recorded for this account.
    Unknown,
    /// The account is actively scheduled for polling.
    Active {
        #[allow(dead_code)]
        next_poll_at: u64,
        #[allow(dead_code)]
        entry: AutomaticDepositCacheEntry,
    },
    /// Polling was stopped after a successful deposit was found. The account
    /// can be rescheduled via `update_balance`.
    Stopped { entry: AutomaticDepositCacheEntry },
    /// The `getSignaturesForAddress` quota for this account has been exhausted.
    /// `update_balance` will return `MonitoringQuotaExhausted` until the manual
    /// flow replenishes the quota.
    Exhausted {
        #[allow(dead_code)]
        entry: AutomaticDepositCacheEntry,
    },
}

pub trait AutomaticDepositCacheExt {
    /// Returns the current monitoring state of the given account.
    fn monitoring_state(&self, account: &Account) -> AccountMonitoringState;
}

impl AutomaticDepositCacheExt for AutomaticDepositCache {
    fn monitoring_state(&self, account: &Account) -> AccountMonitoringState {
        match self.get_with_index(account) {
            None => AccountMonitoringState::Unknown,
            Some((t, entry)) if t != u64::MAX => AccountMonitoringState::Active {
                next_poll_at: t,
                entry,
            },
            Some((_, entry)) if entry.sig_calls_remaining == 0 => {
                AccountMonitoringState::Exhausted { entry }
            }
            Some((_, entry)) => AccountMonitoringState::Stopped { entry },
        }
    }
}
