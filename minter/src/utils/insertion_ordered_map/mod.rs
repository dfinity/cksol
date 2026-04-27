use crate::utils::sorted_key_map::{self, SortedKeyMap};

#[cfg(test)]
mod tests;

/// A map that preserves insertion order while providing O(log n) key lookup.
///
/// Backed by a [`SortedKeyMap`] keyed on an auto-incrementing sequence number,
/// so iteration via [`iter`], [`keys`], and [`values`] is in insertion order (oldest first).
/// [`DoubleEndedIterator`] is supported on [`Iter`], so callers can call `.rev()` on
/// [`iter`] to get newest-first.
///
/// [`iter`]: InsertionOrderedMap::iter
/// [`keys`]: InsertionOrderedMap::keys
/// [`values`]: InsertionOrderedMap::values
pub struct InsertionOrderedMap<K, V> {
    inner: SortedKeyMap<K, u64, V>,
    next_seq: u64,
}

impl<K: Ord + Clone, V> InsertionOrderedMap<K, V> {
    pub fn new() -> Self {
        Self {
            inner: SortedKeyMap::new(),
            next_seq: 0,
        }
    }

    /// Inserts a key-value pair. Returns the old value if the key was already present
    /// (and moves it to the end of the insertion order).
    pub fn insert(&mut self, key: K, value: V) -> Option<V> {
        let seq = self.next_seq;
        self.next_seq += 1;
        self.inner.insert(key, seq, value)
    }

    /// Removes a key and returns its value if it was present.
    pub fn remove(&mut self, key: &K) -> Option<V> {
        self.inner.remove(key)
    }

    pub fn get(&self, key: &K) -> Option<&V> {
        self.inner.get(key)
    }

    pub fn contains_key(&self, key: &K) -> bool {
        self.inner.contains_key(key)
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Returns an iterator over `(&K, &V)` pairs in insertion order.
    pub fn iter(&self) -> Iter<'_, K, V> {
        Iter(self.inner.iter())
    }

    /// Returns an iterator over keys in insertion order.
    pub fn keys(&self) -> Keys<'_, K, V> {
        Keys(self.iter())
    }

    /// Returns an iterator over values in insertion order.
    pub fn values(&self) -> Values<'_, K, V> {
        Values(self.iter())
    }

    /// Returns a mutable iterator over values. Iteration order is unspecified.
    pub fn values_mut(&mut self) -> impl Iterator<Item = &mut V> + '_ {
        self.inner.values_mut()
    }
}

impl<K: Ord + Clone, V> Default for InsertionOrderedMap<K, V> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K: Ord + Clone, V: PartialEq> PartialEq for InsertionOrderedMap<K, V> {
    fn eq(&self, other: &Self) -> bool {
        self.iter().eq(other.iter())
    }
}

impl<K: Ord + Clone, V: Eq> Eq for InsertionOrderedMap<K, V> {}

impl<K: Ord + Clone + std::fmt::Debug, V: std::fmt::Debug> std::fmt::Debug
    for InsertionOrderedMap<K, V>
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_map().entries(self.iter()).finish()
    }
}

impl<'a, K: Ord + Clone, V> IntoIterator for &'a InsertionOrderedMap<K, V> {
    type Item = (&'a K, &'a V);
    type IntoIter = Iter<'a, K, V>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

// --- Iterator types ---

pub struct Iter<'a, K, V>(sorted_key_map::Iter<'a, K, u64, V>);

impl<'a, K: Ord, V> Iterator for Iter<'a, K, V> {
    type Item = (&'a K, &'a V);

    fn next(&mut self) -> Option<Self::Item> {
        self.0.next().map(|(_, k, v)| (k, v))
    }
}

impl<'a, K: Ord, V> DoubleEndedIterator for Iter<'a, K, V> {
    fn next_back(&mut self) -> Option<Self::Item> {
        self.0.next_back().map(|(_, k, v)| (k, v))
    }
}

pub struct Keys<'a, K, V>(Iter<'a, K, V>);

impl<'a, K: Ord, V> Iterator for Keys<'a, K, V> {
    type Item = &'a K;

    fn next(&mut self) -> Option<Self::Item> {
        self.0.next().map(|(k, _)| k)
    }
}

pub struct Values<'a, K, V>(Iter<'a, K, V>);

impl<'a, K: Ord, V> Iterator for Values<'a, K, V> {
    type Item = &'a V;

    fn next(&mut self) -> Option<Self::Item> {
        self.0.next().map(|(_, v)| v)
    }
}
