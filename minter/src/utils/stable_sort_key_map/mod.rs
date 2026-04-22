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
/// - `by_index`: drives [`iter`].
///
// TODO: simplify value/key types to 2-tuples once ic-stable-structures supports 2-tuples
// with one unbounded element.
///
/// [`iter`]: StableSortKeyMap::iter
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

    /// Iterates all `(index, key, value)` triples in ascending index order.
    ///
    /// To iterate only entries up to a given index bound, use standard iterator
    /// adapters: `map.iter().take_while(|(i, ..)| *i <= max)`.
    pub fn iter(&self) -> Iter<'_, K, I, V> {
        Iter {
            index_iter: self.by_index.iter(),
            by_key: &self.by_key,
        }
    }

    pub fn len(&self) -> usize {
        self.by_key.len() as usize
    }

    pub fn is_empty(&self) -> bool {
        self.by_key.is_empty()
    }
}

/// Iterator over `(index, key, value)` triples in ascending index order.
pub struct Iter<'a, K, I, V>
where
    K: Storable + Ord + Clone,
    I: Storable + Ord + Clone,
    V: Storable + Clone,
{
    index_iter: ic_stable_structures::btreemap::Iter<'a, (I, K, ()), (), Memory>,
    by_key: &'a StableBTreeMap<K, (I, V, ()), Memory>,
}

impl<'a, K, I, V> Iterator for Iter<'a, K, I, V>
where
    K: Storable + Ord + Clone,
    I: Storable + Ord + Clone,
    V: Storable + Clone,
{
    type Item = (I, K, V);

    fn next(&mut self) -> Option<Self::Item> {
        let ((i, k, _), _) = self.index_iter.next()?;
        let (_, v, _) = self
            .by_key
            .get(&k)
            .expect("index and by_key map must be in sync");
        Some((i, k, v))
    }
}
