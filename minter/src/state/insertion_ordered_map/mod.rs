use std::collections::BTreeMap;

#[cfg(test)]
mod tests;

/// A map that preserves insertion order while providing O(log n) key lookup.
///
/// Internally uses two `BTreeMap`s:
/// - `entries`: key → (sequence number, value)
/// - `order`: sequence number → key
///
/// Iteration via [`iter`], [`keys`], and [`values`] is in insertion order (oldest first).
///
/// [`iter`]: InsertionOrderedMap::iter
/// [`keys`]: InsertionOrderedMap::keys
/// [`values`]: InsertionOrderedMap::values
pub struct InsertionOrderedMap<K, V> {
    entries: BTreeMap<K, (u64, V)>,
    order: BTreeMap<u64, K>,
    next_seq: u64,
}

impl<K: Ord + Clone, V> InsertionOrderedMap<K, V> {
    pub fn new() -> Self {
        Self {
            entries: BTreeMap::new(),
            order: BTreeMap::new(),
            next_seq: 0,
        }
    }

    /// Inserts a key-value pair. Returns the old value if the key was already present
    /// (and moves it to the end of the insertion order).
    pub fn insert(&mut self, key: K, value: V) -> Option<V> {
        let old_value = if let Some((old_seq, old_val)) = self.entries.remove(&key) {
            self.order.remove(&old_seq);
            Some(old_val)
        } else {
            None
        };
        let seq = self.next_seq;
        self.next_seq += 1;
        self.order.insert(seq, key.clone());
        self.entries.insert(key, (seq, value));
        old_value
    }

    /// Removes a key and returns its value if it was present.
    pub fn remove(&mut self, key: &K) -> Option<V> {
        if let Some((seq, value)) = self.entries.remove(key) {
            self.order.remove(&seq);
            Some(value)
        } else {
            None
        }
    }

    pub fn get(&self, key: &K) -> Option<&V> {
        self.entries.get(key).map(|(_, v)| v)
    }

    pub fn contains_key(&self, key: &K) -> bool {
        self.entries.contains_key(key)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Returns an iterator over `(&K, &V)` pairs in insertion order.
    pub fn iter(&self) -> Iter<'_, K, V> {
        Iter {
            order_iter: self.order.values(),
            entries: &self.entries,
        }
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
        self.entries.values_mut().map(|(_, v)| v)
    }

    /// Removes all entries for which `pred` returns `true` and returns them.
    pub fn extract_if<F: FnMut(&K, &V) -> bool>(&mut self, mut pred: F) -> Vec<(K, V)> {
        let matching_keys: Vec<K> = self
            .entries
            .iter()
            .filter_map(|(k, (_, v))| if pred(k, v) { Some(k.clone()) } else { None })
            .collect();
        let mut result = Vec::with_capacity(matching_keys.len());
        for key in matching_keys {
            if let Some((seq, value)) = self.entries.remove(&key) {
                self.order.remove(&seq);
                result.push((key, value));
            }
        }
        result
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

pub struct Iter<'a, K, V> {
    order_iter: std::collections::btree_map::Values<'a, u64, K>,
    entries: &'a BTreeMap<K, (u64, V)>,
}

impl<'a, K: Ord, V> Iterator for Iter<'a, K, V> {
    type Item = (&'a K, &'a V);

    fn next(&mut self) -> Option<Self::Item> {
        let key = self.order_iter.next()?;
        let (_, value) = self.entries.get(key)?;
        Some((key, value))
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
