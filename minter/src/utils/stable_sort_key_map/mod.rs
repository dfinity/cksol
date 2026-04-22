use ic_stable_structures::{
    DefaultMemoryImpl, StableBTreeMap, Storable, memory_manager::VirtualMemory,
};

type Memory = VirtualMemory<DefaultMemoryImpl>;

#[cfg(test)]
mod tests;

/// A stable-memory map with a secondary sort index per entry.
///
/// Two `StableBTreeMap`s are kept in sync:
/// - `by_key`: primary store, always contains every entry.
/// - `by_index`: drives [`peek`] and [`iter_by_index_up_to`].
///
// TODO: simplify value/key types to 2-tuples once ic-stable-structures supports 2-tuples
// with one unbounded element.
///
/// [`peek`]: StableSortKeyMap::peek
/// [`iter_by_index_up_to`]: StableSortKeyMap::iter_by_index_up_to
/// [`get`]: StableSortKeyMap::get
pub struct StableSortKeyMap<K, I, V>
where
    K: Storable + Ord + Clone,
    I: Storable + Ord + Clone,
    V: Storable + Clone,
{
    by_key: StableBTreeMap<K, (I, V, ()), Memory>,
    by_index: StableBTreeMap<(I, K, ()), (), Memory>,
}

impl<K, I, V> StableSortKeyMap<K, I, V>
where
    K: Storable + Ord + Clone,
    I: Storable + Ord + Clone,
    V: Storable + Clone,
{
    /// Initializes the map from existing stable memory (used in production / post-upgrade).
    pub fn init(by_key_mem: Memory, by_index_mem: Memory) -> Self {
        Self {
            by_key: StableBTreeMap::init(by_key_mem),
            by_index: StableBTreeMap::init(by_index_mem),
        }
    }

    /// Creates an empty map in the given memory regions (used in tests).
    pub fn new(by_key_mem: Memory, by_index_mem: Memory) -> Self {
        Self {
            by_key: StableBTreeMap::new(by_key_mem),
            by_index: StableBTreeMap::new(by_index_mem),
        }
    }

    /// Returns the value for the given key, or `None` if absent.
    pub fn get(&self, key: &K) -> Option<V> {
        self.by_key.get(key).map(|(_, v, _)| v)
    }

    /// Returns the current index and value for the given key, or `None` if absent.
    pub fn get_with_index(&self, key: &K) -> Option<(I, V)> {
        self.by_key.get(key).map(|(i, v, _)| (i, v))
    }

    /// Inserts or updates an entry.
    pub fn insert(&mut self, key: K, index: I, value: V) {
        if let Some((old_index, _, _)) = self.by_key.get(&key) {
            self.by_index.remove(&(old_index, key.clone(), ()));
        }
        self.by_index.insert((index.clone(), key.clone(), ()), ());
        self.by_key.insert(key, (index, value, ()));
    }

    /// Updates the index and value for an existing entry.
    ///
    /// Unlike [`insert`], this communicates that the entry already exists and
    /// its sort position (index) is being updated — e.g. to reschedule a poll.
    ///
    /// # Panics
    ///
    /// Panics if `key` is not present in the map.
    ///
    /// [`insert`]: Self::insert
    pub fn update_index(&mut self, key: K, new_index: I, new_value: V) {
        let (old_index, _, _) = self
            .by_key
            .get(&key)
            .expect("update_index called for non-existent key");
        self.by_index.remove(&(old_index, key.clone(), ()));
        self.by_index
            .insert((new_index.clone(), key.clone(), ()), ());
        self.by_key.insert(key, (new_index, new_value, ()));
    }

    /// Returns the `(index, key)` of the entry with the smallest index, if any.
    ///
    /// O(log n).
    pub fn peek(&self) -> Option<(I, K)> {
        self.by_index.iter().next().map(|((i, k, _), _)| (i, k))
    }

    /// Iterates `(key, value)` pairs in ascending index order, stopping at
    /// the first entry whose index exceeds `max` (inclusive bound).
    pub fn iter_by_index_up_to<'a>(&'a self, max: &'a I) -> impl Iterator<Item = (K, V)> + 'a {
        let by_key = &self.by_key;
        self.by_index
            .iter()
            .take_while(move |((i, _, _), _)| i <= max)
            .map(move |((_, k, _), _)| {
                let (_, v, _) = by_key
                    .get(&k)
                    .expect("index and by_key map must be in sync");
                (k, v)
            })
    }

    pub fn len(&self) -> usize {
        self.by_key.len() as usize
    }

    pub fn is_empty(&self) -> bool {
        self.by_key.is_empty()
    }
}
