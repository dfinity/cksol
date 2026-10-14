use crate::utils::timed_sized_map::{Entry, InsertError, TimedSizedMap, Timestamp};
use icrc_ledger_types::icrc1::account::Account;
use solana_address::Address;
use std::{num::NonZeroUsize, time::Duration};

#[cfg(test)]
mod tests;

/// How long a registration keeps a deposit address watched.
pub const WATCH_WINDOW: Duration = Duration::from_hours(24);

/// The number of live registrations the watchlist holds at most.
pub const MAX_WATCHED_ADDRESSES: NonZeroUsize =
    NonZeroUsize::new(1_000).expect("BUG: the watchlist capacity must be non-zero");

/// The gaps between a scan and the next one for the first scans, indexed by the number of
/// scans done so far minus one: a burst of ten scans within the first half hour.
const BURST_SCAN_GAPS: [Duration; 9] = [
    Duration::from_secs(60),
    Duration::from_secs(60),
    Duration::from_secs(120),
    Duration::from_secs(120),
    Duration::from_secs(240),
    Duration::from_secs(300),
    Duration::from_secs(300),
    Duration::from_secs(300),
    Duration::from_secs(300),
];

/// The gap between a scan and the next one after the burst, until the window closes.
const STEADY_SCAN_GAP: Duration = Duration::from_hours(1);

/// The deposit addresses registered for the automatic deposit flow, each watched for
/// [`WATCH_WINDOW`] and scanned on the schedule of [`BURST_SCAN_GAPS`] and [`STEADY_SCAN_GAP`].
#[derive(Debug, PartialEq, Eq)]
pub struct Watchlist {
    entries: TimedSizedMap<Account, ScanProgress>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ScanProgress {
    address: Address,
    scan_count: u32,
    last_scanned_at: Option<Timestamp>,
}

impl ScanProgress {
    fn unscanned(address: Address) -> Self {
        Self {
            address,
            scan_count: 0,
            last_scanned_at: None,
        }
    }

    fn next_scan_at(&self, now: Timestamp, valid_until: Timestamp) -> Option<Timestamp> {
        let due_at = match self.last_scanned_at {
            None => now,
            Some(last_scanned_at) => {
                let gap = BURST_SCAN_GAPS
                    .get(self.scan_count as usize - 1)
                    .copied()
                    .unwrap_or(STEADY_SCAN_GAP);
                last_scanned_at.saturating_add(gap)
            }
        };
        (due_at <= valid_until).then_some(due_at)
    }
}

/// A registration of the watchlist with its scan progress, as of the time it was read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WatchedAddress {
    pub account: Account,
    pub address: Address,
    pub valid_until: Timestamp,
    pub scan_count: u32,
    pub last_scanned_at: Option<Timestamp>,
    /// When the next scan is due: at once for an address not scanned yet, at the end of the
    /// gap after its last scan otherwise, or `None` once the schedule leaves no scan before
    /// the window closes.
    pub next_scan_at: Option<Timestamp>,
}

/// The watchlist is full of live registrations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WatchlistFull;

impl Default for Watchlist {
    fn default() -> Self {
        Self {
            entries: TimedSizedMap::new(WATCH_WINDOW, MAX_WATCHED_ADDRESSES),
        }
    }
}

impl Watchlist {
    /// Registers the deposit address of the account for a window starting now, or returns the
    /// live registration of the account as it is, without extending it.
    pub fn register(
        &mut self,
        now: Timestamp,
        account: Account,
        address: Address,
    ) -> Result<WatchedAddress, WatchlistFull> {
        match self
            .entries
            .insert(now, account, ScanProgress::unscanned(address))
        {
            Ok(_) | Err(InsertError::AlreadyPresent { .. }) => Ok(self
                .get(now, &account)
                .expect("BUG: a registration is live right after its insertion")),
            Err(InsertError::AtCapacity { .. }) => Err(WatchlistFull),
            Err(InsertError::Expired { .. }) => {
                unreachable!(
                    "BUG: a registration for the window starting now never expires on insertion"
                )
            }
        }
    }

    /// The live registration of the account, if any.
    pub fn get(&self, now: Timestamp, account: &Account) -> Option<WatchedAddress> {
        let entry = self.entries.get_entry(now, account)?;
        Some(self.watched(now, account, entry))
    }

    pub fn remove(&mut self, account: &Account) {
        self.entries.remove(account);
    }

    /// The live registrations whose next scan is due.
    pub fn due(&self, now: Timestamp) -> Vec<WatchedAddress> {
        self.live(now)
            .filter(|watched| watched.next_scan_at.is_some_and(|due_at| due_at <= now))
            .collect()
    }

    /// Records a scan of the account's deposit address at `now`, which advances the account
    /// along the schedule, if the account is still watched.
    pub fn record_scan(&mut self, now: Timestamp, account: &Account) {
        if let Some(progress) = self.entries.get_value_mut(now, account) {
            progress.scan_count = progress.scan_count.saturating_add(1);
            progress.last_scanned_at = Some(now);
        }
    }

    /// Removes the registrations whose window has closed and returns how many there were.
    pub fn evict_expired(&mut self, now: Timestamp) -> usize {
        self.entries.evict_expired(now).len()
    }

    /// Every registration, live or expired but not yet evicted, by ascending window end.
    pub fn iter(&self, now: Timestamp) -> impl Iterator<Item = WatchedAddress> {
        self.entries
            .iter_by_expiry()
            .map(move |(account, entry)| self.watched(now, account, entry))
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn live_len(&self, now: Timestamp) -> usize {
        self.live(now).count()
    }

    /// When the oldest live registration was made, if any.
    pub fn oldest_registered_at(&self, now: Timestamp) -> Option<Timestamp> {
        self.live(now)
            .map(|watched| self.registered_at(watched.valid_until))
            .min()
    }

    fn live(&self, now: Timestamp) -> impl Iterator<Item = WatchedAddress> {
        self.entries
            .iter_by_expiry()
            .filter(move |(_, entry)| entry.is_live(now))
            .map(move |(account, entry)| self.watched(now, account, entry))
    }

    fn watched(
        &self,
        now: Timestamp,
        account: &Account,
        entry: &Entry<ScanProgress>,
    ) -> WatchedAddress {
        let progress = &entry.value;
        WatchedAddress {
            account: *account,
            address: progress.address,
            valid_until: entry.expires_at,
            scan_count: progress.scan_count,
            last_scanned_at: progress.last_scanned_at,
            next_scan_at: progress.next_scan_at(now, entry.expires_at),
        }
    }

    fn registered_at(&self, valid_until: Timestamp) -> Timestamp {
        valid_until.saturating_sub(self.entries.ttl())
    }
}
