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

/// The slots of the first scans, measured from the start of the schedule: a burst of ten
/// scans within the first half hour.
const BURST_SCAN_SLOTS: [Duration; 10] = [
    Duration::ZERO,
    Duration::from_secs(60),
    Duration::from_secs(120),
    Duration::from_secs(240),
    Duration::from_secs(360),
    Duration::from_secs(600),
    Duration::from_secs(900),
    Duration::from_secs(1_200),
    Duration::from_secs(1_500),
    Duration::from_secs(1_800),
];

/// The gap between two slots after the burst, until the window closes.
const STEADY_SCAN_GAP: Duration = Duration::from_hours(1);

/// The deposit addresses registered for the automatic deposit flow, each watched for
/// [`WATCH_WINDOW`] and scanned at the slots of [`BURST_SCAN_SLOTS`], then every
/// [`STEADY_SCAN_GAP`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Watchlist {
    entries: TimedSizedMap<Account, ScanProgress>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ScanProgress {
    address: Address,
    scheduled_from: Timestamp,
    scan_count: u32,
    last_scanned_at: Option<Timestamp>,
}

impl ScanProgress {
    fn unscanned(address: Address, scheduled_from: Timestamp) -> Self {
        Self {
            address,
            scheduled_from,
            scan_count: 0,
            last_scanned_at: None,
        }
    }

    /// The first slot of the schedule before any scan and the first slot after the last scan
    /// otherwise, unless no slot is left before the end of the window: the slots are fixed, so
    /// a late scan does not shift the ones that follow and the slots it skipped are not made up.
    fn next_scan_at(&self, valid_until: Timestamp) -> Option<Timestamp> {
        let mut slots = scan_slots()
            .map(|slot| self.scheduled_from.saturating_add(slot))
            .take_while(|slot_at| *slot_at <= valid_until);
        match self.last_scanned_at {
            None => slots.next(),
            Some(last_scanned_at) => slots.find(|slot_at| *slot_at > last_scanned_at),
        }
    }
}

/// The slots of the schedule in order, as offsets from its start, up to the length of a window.
fn scan_slots() -> impl Iterator<Item = Duration> {
    let last_burst_slot = BURST_SCAN_SLOTS[BURST_SCAN_SLOTS.len() - 1];
    let steady_slots = (1..).map(move |gaps| last_burst_slot + STEADY_SCAN_GAP * gaps);
    BURST_SCAN_SLOTS
        .into_iter()
        .chain(steady_slots)
        .take_while(|slot| *slot <= WATCH_WINDOW)
}

/// A registration of the watchlist with its scan progress, as of the time it was read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WatchedAddress {
    pub account: Account,
    pub address: Address,
    pub valid_until: Timestamp,
    pub scan_count: u32,
    pub last_scanned_at: Option<Timestamp>,
    /// When the next scan is due: the first slot of the schedule for an address not scanned
    /// yet, the first slot after its last scan otherwise, or `None` once no slot is left
    /// before the window closes.
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
    /// Registers the deposit address of the account for a window and a schedule starting now,
    /// or returns the live registration of the account as it is, without extending it.
    pub fn register(
        &mut self,
        now: Timestamp,
        account: Account,
        address: Address,
    ) -> Result<WatchedAddress, WatchlistFull> {
        match self
            .entries
            .insert(now, account, ScanProgress::unscanned(address, now))
        {
            Ok(_) | Err(InsertError::AlreadyPresent { .. }) => Ok(self
                .get(now, &account)
                .expect("BUG: a registration is live right after its insertion")),
            Err(InsertError::AtCapacity { .. }) => Err(WatchlistFull),
        }
    }

    /// The live registration of the account, if any.
    pub fn get(&self, now: Timestamp, account: &Account) -> Option<WatchedAddress> {
        let entry = self.entries.get_entry(now, account)?;
        Some(Self::watched(account, entry))
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
    /// to the first slot of its schedule after `now`, if the account is still watched.
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
    pub fn iter(&self) -> impl Iterator<Item = WatchedAddress> {
        self.entries
            .iter_by_expiry()
            .map(|(account, entry)| Self::watched(account, entry))
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
            .map(|(account, entry)| Self::watched(account, entry))
    }

    fn watched(account: &Account, entry: &Entry<ScanProgress>) -> WatchedAddress {
        let progress = &entry.value;
        WatchedAddress {
            account: *account,
            address: progress.address,
            valid_until: entry.expires_at,
            scan_count: progress.scan_count,
            last_scanned_at: progress.last_scanned_at,
            next_scan_at: progress.next_scan_at(entry.expires_at),
        }
    }

    fn registered_at(&self, valid_until: Timestamp) -> Timestamp {
        valid_until.saturating_sub(self.entries.ttl())
    }
}
