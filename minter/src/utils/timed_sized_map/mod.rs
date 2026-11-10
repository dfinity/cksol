use std::{
    collections::{BTreeMap, VecDeque},
    num::NonZeroUsize,
    time::Duration,
};

#[cfg(test)]
mod tests;

/// Nanoseconds since the Unix epoch, as returned by `ic_cdk::api::time()`.
#[derive(Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash, Debug)]
pub struct Timestamp(u64);

impl Timestamp {
    pub const fn from_nanos(nanos: u64) -> Self {
        Self(nanos)
    }

    pub const fn as_nanos(self) -> u64 {
        self.0
    }

    pub fn saturating_add(self, duration: Duration) -> Self {
        Self(self.0.saturating_add(nanos(duration)))
    }

    pub fn saturating_sub(self, duration: Duration) -> Self {
        Self(self.0.saturating_sub(nanos(duration)))
    }
}

fn nanos(duration: Duration) -> u64 {
    u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX)
}

/// A value of a [`TimedSizedMap`] together with the time after which it is expired.
#[derive(Clone, Eq, PartialEq, Debug)]
pub struct Entry<V> {
    pub value: V,
    pub expires_at: Timestamp,
}

impl<V> Entry<V> {
    /// Whether the entry is live at `now`, which it is through its expiry instant.
    pub fn is_live(&self, now: Timestamp) -> bool {
        !is_expired(self.expires_at, now)
    }
}

/// A map of at most `capacity` entries, each living for at most `ttl`.
///
/// Re-inserting a key that still has a live entry is rejected, leaving the map untouched.
/// Admitting a new key first evicts the entries that have outlived their `ttl`; if the map
/// is still at capacity with live entries only, the insertion is rejected rather than
/// evicting a live entry. Time is supplied by the caller on every operation, so the map
/// holds no clock and is deterministic.
#[derive(Clone, Eq, PartialEq, Debug)]
pub struct TimedSizedMap<K, V> {
    ttl: Duration,
    capacity: NonZeroUsize,
    entries: BTreeMap<K, Entry<V>>,
    by_expiry: BTreeMap<Timestamp, VecDeque<K>>,
}

/// Why [`TimedSizedMap::insert`] or [`TimedSizedMap::insert_entry`] did not admit a new
/// entry. The rejected key and value are handed back to the caller.
#[derive(Clone, Eq, PartialEq, Debug)]
pub enum InsertError<K, V> {
    /// A live entry already exists under the key. Refreshing it is not allowed, so that a
    /// caller cannot extend the lifetime of an entry by inserting it again.
    AlreadyPresent { key: K, value: V },
    /// The map is at capacity with live entries only. A live entry is never evicted to make
    /// room, since a still-watched address may already have received funds.
    AtCapacity { key: K, value: V },
}

impl<K: Ord + Clone, V> TimedSizedMap<K, V> {
    pub fn new(ttl: Duration, capacity: NonZeroUsize) -> Self {
        Self {
            ttl,
            capacity,
            entries: BTreeMap::new(),
            by_expiry: BTreeMap::new(),
        }
    }

    /// Rebuilds a map verbatim from its `ttl`, `capacity` and the entries previously produced
    /// by [`Self::iter_by_expiry`]: no expiry clamping, no eviction of expired entries and no
    /// capacity check, so that the result equals the map that produced the entries.
    ///
    /// # Panics
    ///
    /// If a key appears twice, since the entries are not a snapshot of a map then.
    pub fn from_ordered_entries(
        ttl: Duration,
        capacity: NonZeroUsize,
        entries: impl IntoIterator<Item = (K, Entry<V>)>,
    ) -> Self {
        let mut map = Self::new(ttl, capacity);
        for (key, entry) in entries {
            map.enqueue_by_expiry(&key, entry.expires_at);
            assert!(
                map.entries.insert(key, entry).is_none(),
                "BUG: from_ordered_entries received a duplicate key"
            );
        }
        map
    }

    /// Inserts `value` under `key` with a lifetime of `ttl` from `now` and returns the expired
    /// entries evicted to make room.
    pub fn insert(
        &mut self,
        now: Timestamp,
        key: K,
        value: V,
    ) -> Result<Vec<(K, V)>, InsertError<K, V>> {
        let entry = Entry {
            value,
            expires_at: self.expiry(now),
        };
        self.insert_entry(now, key, entry)
    }

    /// Inserts an entry with its own expiry, clamped to `ttl` from `now`, and returns the
    /// expired entries evicted to make room. Inserting an already expired entry is a no-op
    /// that also returns `Ok`, so `Ok` alone does not mean that the entry was inserted.
    pub fn insert_entry(
        &mut self,
        now: Timestamp,
        key: K,
        entry: Entry<V>,
    ) -> Result<Vec<(K, V)>, InsertError<K, V>> {
        if !entry.is_live(now) {
            return Ok(vec![]);
        }
        if self.get(now, &key).is_some() {
            return Err(InsertError::AlreadyPresent {
                key,
                value: entry.value,
            });
        }
        let evicted = self.evict_expired(now);
        if self.entries.len() >= self.capacity.get() {
            return Err(InsertError::AtCapacity {
                key,
                value: entry.value,
            });
        }
        let entry = Entry {
            value: entry.value,
            expires_at: entry.expires_at.min(self.expiry(now)),
        };
        self.enqueue_by_expiry(&key, entry.expires_at);
        self.entries.insert(key, entry);
        Ok(evicted)
    }

    /// The live value under `key`, if any.
    pub fn get(&self, now: Timestamp, key: &K) -> Option<&V> {
        self.get_entry(now, key).map(|entry| &entry.value)
    }

    /// The live entry under `key`, if any.
    pub fn get_entry(&self, now: Timestamp, key: &K) -> Option<&Entry<V>> {
        self.entries.get(key).filter(|entry| entry.is_live(now))
    }

    /// The live value under `key` for modification, if any. The expiry stays out of reach, so
    /// that the expiry index cannot go out of sync.
    pub fn get_value_mut(&mut self, now: Timestamp, key: &K) -> Option<&mut V> {
        self.entries
            .get_mut(key)
            .filter(|entry| entry.is_live(now))
            .map(|entry| &mut entry.value)
    }

    /// Removes and returns every entry that has outlived its `ttl`.
    pub fn evict_expired(&mut self, now: Timestamp) -> Vec<(K, V)> {
        let mut evicted = Vec::new();
        while let Some(expires_at) = self.earliest_expiry()
            && is_expired(expires_at, now)
        {
            let bucket = self
                .by_expiry
                .remove(&expires_at)
                .expect("BUG: the earliest expiry has a bucket");
            for key in bucket {
                let entry = self
                    .entries
                    .remove(&key)
                    .expect("BUG: every indexed key has an entry");
                evicted.push((key, entry.value));
            }
        }
        evicted
    }

    /// Removes and returns the entry under `key`, live or expired.
    pub fn remove(&mut self, key: &K) -> Option<Entry<V>> {
        let entry = self.entries.remove(key)?;
        let bucket = self
            .by_expiry
            .get_mut(&entry.expires_at)
            .expect("BUG: every entry is indexed under its expiry");
        bucket.retain(|indexed| indexed != key);
        if bucket.is_empty() {
            self.by_expiry.remove(&entry.expires_at);
        }
        Some(entry)
    }

    /// Every entry, live or expired, in key order.
    pub fn iter(&self) -> impl Iterator<Item = (&K, &Entry<V>)> {
        self.entries.iter()
    }

    /// Every entry, live or expired, by ascending expiry and insertion order within an expiry,
    /// which is the order [`Self::from_ordered_entries`] rebuilds an identical map from.
    pub fn iter_by_expiry(&self) -> impl Iterator<Item = (&K, &Entry<V>)> {
        self.by_expiry.values().flatten().map(|key| {
            let entry = self
                .entries
                .get(key)
                .expect("BUG: every indexed key has an entry");
            (key, entry)
        })
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn capacity(&self) -> NonZeroUsize {
        self.capacity
    }

    pub fn ttl(&self) -> Duration {
        self.ttl
    }

    fn expiry(&self, now: Timestamp) -> Timestamp {
        now.saturating_add(self.ttl)
    }

    fn earliest_expiry(&self) -> Option<Timestamp> {
        self.by_expiry.keys().next().copied()
    }

    fn enqueue_by_expiry(&mut self, key: &K, expires_at: Timestamp) {
        self.by_expiry
            .entry(expires_at)
            .or_default()
            .push_back(key.clone());
    }
}

fn is_expired(expires_at: Timestamp, now: Timestamp) -> bool {
    now > expires_at
}
