use crate::utils::stable_sort_key_map::StableSortKeyMap;
use ic_stable_structures::{Storable, storable::Bound};
use icrc_ledger_types::icrc1::account::Account;
use minicbor::{Decode, Encode};
use std::borrow::Cow;

#[cfg(test)]
mod tests;

/// Initial RPC call quota granted to each monitored account.
pub const INITIAL_RPC_QUOTA: u64 = 50;

/// Initial backoff delay in minutes before the first poll.
pub const INITIAL_BACKOFF_DELAY_MINS: u64 = 1;

/// Per-account state for automated deposit discovery.
///
/// This cache is intentionally separate from the event log: it can be fully
/// reconstructed by redoing the `getSignaturesForAddress` HTTP outcalls, so
/// there is no need to replay events to restore it.
#[derive(Clone, Debug, PartialEq, Encode, Decode)]
pub struct AutomaticDepositCacheEntry {
    /// The number of RPC calls remaining for this account.
    #[n(0)]
    pub rpc_quota_left: u64,
    /// The delay in minutes before the next poll. Doubles after each poll.
    #[n(1)]
    pub next_backoff_delay_mins: u64,
}

impl Default for AutomaticDepositCacheEntry {
    fn default() -> Self {
        Self {
            rpc_quota_left: INITIAL_RPC_QUOTA,
            next_backoff_delay_mins: INITIAL_BACKOFF_DELAY_MINS,
        }
    }
}

impl Storable for AutomaticDepositCacheEntry {
    fn to_bytes(&self) -> Cow<'_, [u8]> {
        let mut buf = vec![];
        minicbor::encode(self, &mut buf)
            .expect("AutomaticDepositCacheEntry encoding should succeed");
        Cow::Owned(buf)
    }

    fn from_bytes(bytes: Cow<[u8]>) -> Self {
        minicbor::decode(bytes.as_ref()).unwrap_or_else(|e| {
            panic!(
                "failed to decode AutomaticDepositCacheEntry: {e} (bytes: {})",
                hex::encode(bytes)
            )
        })
    }

    const BOUND: Bound = Bound::Unbounded;
}

/// Map from `Account` to `AutomaticDepositCacheEntry`, indexed by `next_poll_at`
/// timestamp for ordered iteration. The poll time is stored alongside the cache entry
/// inside the map (not in `AutomaticDepositCacheEntry` itself), analogous to how
/// `InsertionOrderedMap` stores the sequence number alongside the value.
///
/// Accounts that have been stopped from monitoring are stored with index `u64::MAX`
/// so they are never scheduled for another poll.
pub type AutomaticDepositCache = StableSortKeyMap<Account, u64, AutomaticDepositCacheEntry>;

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
    /// Polling was stopped after the quota was exhausted, but a subsequent deposit
    /// reset the quota. The account can be rescheduled via `update_balance`.
    Stopped { entry: AutomaticDepositCacheEntry },
    /// The RPC quota for this account has been exhausted and no deposit has reset it.
    /// `update_balance` will return `MonitoringQuotaExhausted` until a deposit resets
    /// the quota.
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
            Some((_, entry)) if entry.rpc_quota_left == 0 => {
                AccountMonitoringState::Exhausted { entry }
            }
            Some((_, entry)) => AccountMonitoringState::Stopped { entry },
        }
    }
}
