use std::collections::BTreeMap;

#[cfg(test)]
mod tests;

/// A map with a user-supplied sort index, backed by two `BTreeMap`s.
///
/// Two `BTreeMap`s are kept in sync:
/// - `by_key`: primary store, key → (index, value)
/// - `by_index`: secondary index, (index, key) → (), drives [`iter`] in ascending index order
///
/// Unlike a secondary index keyed only by `I`, composite `(I, K)` keys allow
/// multiple entries to share the same index value (e.g. several accounts scheduled
/// at the same timestamp).
///
/// [`iter`]: SortedKeyMap::iter
pub struct SortedKeyMap<K, I, V> {
    by_key: BTreeMap<K, (I, V)>,
    by_index: BTreeMap<(I, K), ()>,
}

impl<K, I, V> Default for SortedKeyMap<K, I, V> {
    fn default() -> Self {
        Self {
            by_key: BTreeMap::new(),
            by_index: BTreeMap::new(),
        }
    }
}

impl<K: Ord + Clone, I: Ord + Clone, V> SortedKeyMap<K, I, V> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Inserts or updates an entry, atomically updating the sort index.
    /// Returns the old value if the key was already present.
    pub fn insert(&mut self, key: K, index: I, value: V) -> Option<V> {
        let old_value = if let Some((old_index, old_val)) = self.by_key.remove(&key) {
            self.by_index.remove(&(old_index, key.clone()));
            Some(old_val)
        } else {
            None
        };
        self.by_index.insert((index.clone(), key.clone()), ());
        self.by_key.insert(key, (index, value));
        old_value
    }

    /// Removes a key and returns its value if present.
    pub fn remove(&mut self, key: &K) -> Option<V> {
        if let Some((index, value)) = self.by_key.remove(key) {
            self.by_index.remove(&(index, key.clone()));
            Some(value)
        } else {
            None
        }
    }

    /// Returns a reference to the value for the given key.
    pub fn get(&self, key: &K) -> Option<&V> {
        self.by_key.get(key).map(|(_, v)| v)
    }

    /// Returns references to the index and value for the given key.
    pub fn get_with_index(&self, key: &K) -> Option<(&I, &V)> {
        self.by_key.get(key).map(|(i, v)| (i, v))
    }

    pub fn contains_key(&self, key: &K) -> bool {
        self.by_key.contains_key(key)
    }

    /// Iterates `(index, key, value)` triples in ascending index order.
    ///
    /// Entries with equal index values are ordered by key.
    pub fn iter(&self) -> Iter<'_, K, I, V> {
        Iter {
            index_iter: self.by_index.keys(),
            by_key: &self.by_key,
        }
    }

    /// Returns a mutable iterator over values. Iteration order is unspecified.
    pub fn values_mut(&mut self) -> impl Iterator<Item = &mut V> + '_ {
        self.by_key.values_mut().map(|(_, v)| v)
    }

    pub fn len(&self) -> usize {
        self.by_key.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_key.is_empty()
    }
}

/// Iterator over `(index, key, value)` triples in ascending index order.
pub struct Iter<'a, K, I, V> {
    index_iter: std::collections::btree_map::Keys<'a, (I, K), ()>,
    by_key: &'a BTreeMap<K, (I, V)>,
}

impl<'a, K: Ord, I: Ord, V> Iterator for Iter<'a, K, I, V> {
    type Item = (&'a I, &'a K, &'a V);

    fn next(&mut self) -> Option<Self::Item> {
        let (i, k) = self.index_iter.next()?;
        let (_, v) = self
            .by_key
            .get(k)
            .expect("by_index and by_key must be in sync");
        Some((i, k, v))
    }
}

impl<'a, K: Ord, I: Ord, V> DoubleEndedIterator for Iter<'a, K, I, V> {
    fn next_back(&mut self) -> Option<Self::Item> {
        let (i, k) = self.index_iter.next_back()?;
        let (_, v) = self
            .by_key
            .get(k)
            .expect("by_index and by_key must be in sync");
        Some((i, k, v))
    }
}
