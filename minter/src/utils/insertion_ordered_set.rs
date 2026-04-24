use super::insertion_ordered_map::InsertionOrderedMap;

/// An insertion-ordered set backed by an [`InsertionOrderedMap`].
///
/// Provides O(log n) membership tests and O(log n) insertion/removal,
/// while preserving insertion order during iteration.
pub struct InsertionOrderedSet<K>(InsertionOrderedMap<K, ()>);

impl<K: Ord + Clone> InsertionOrderedSet<K> {
    pub fn new() -> Self {
        Self(InsertionOrderedMap::new())
    }

    /// Inserts `key`. Returns `true` if the key was not already present.
    pub fn insert(&mut self, key: K) -> bool {
        self.0.insert(key, ()).is_none()
    }

    /// Removes `key`. Returns `true` if the key was present.
    pub fn remove(&mut self, key: &K) -> bool {
        self.0.remove(key).is_some()
    }

    pub fn contains(&self, key: &K) -> bool {
        self.0.contains_key(key)
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Iterates over keys in insertion order.
    pub fn iter(&self) -> impl Iterator<Item = &K> {
        self.0.keys()
    }
}

impl<K: Ord + Clone> Default for InsertionOrderedSet<K> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K: Ord + Clone> PartialEq for InsertionOrderedSet<K> {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl<K: Ord + Clone> Eq for InsertionOrderedSet<K> {}

impl<K: Ord + Clone + std::fmt::Debug> std::fmt::Debug for InsertionOrderedSet<K> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_set().entries(self.iter()).finish()
    }
}
