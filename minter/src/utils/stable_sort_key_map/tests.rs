use super::*;
use ic_stable_structures::{
    DefaultMemoryImpl,
    memory_manager::{MemoryId, MemoryManager},
    storable::Bound,
};
use std::borrow::Cow;

// --- Test fixtures ---

#[derive(Clone, PartialEq, Debug)]
struct Entry {
    data: u32,
}

impl Storable for Entry {
    fn to_bytes(&self) -> Cow<'_, [u8]> {
        Cow::Owned(self.data.to_be_bytes().to_vec())
    }

    fn from_bytes(bytes: Cow<[u8]>) -> Self {
        Self {
            data: u32::from_be_bytes(bytes[..4].try_into().unwrap()),
        }
    }

    const BOUND: Bound = Bound::Bounded {
        max_size: 4,
        is_fixed_size: true,
    };
}

fn make_map() -> StableSortKeyMap<u64, u64, Entry> {
    let mm = MemoryManager::init(DefaultMemoryImpl::default());
    StableSortKeyMap::new(mm.get(MemoryId::new(0)), mm.get(MemoryId::new(1)))
}

fn entry(data: u32) -> Entry {
    Entry { data }
}

// --- Tests ---

mod insert {
    use super::*;

    #[test]
    fn should_store_value_by_key() {
        let mut map = make_map();
        map.insert(1, 100, entry(42));
        assert_eq!(map.get(&1), Some(entry(42)));
        assert_eq!(map.len(), 1);
    }

    #[test]
    fn should_index_entry() {
        let mut map = make_map();
        map.insert(1, 50, entry(0));
        let mut iter = map.iter();
        assert_eq!(iter.next(), Some((50, 1, entry(0))));
        assert_eq!(iter.next(), None);
    }

    #[test]
    fn should_update_index_when_index_changes() {
        let mut map = make_map();
        map.insert(1, 100, entry(0));
        map.insert(1, 200, entry(1));
        assert_eq!(map.get_with_index(&1), Some((200, entry(1))));
        assert_eq!(map.len(), 1);
    }
}

mod get_with_index {
    use super::*;

    #[test]
    fn should_return_index_and_value() {
        let mut map = make_map();
        map.insert(1, 99, entry(7));
        assert_eq!(map.get_with_index(&1), Some((99, entry(7))));
    }

    #[test]
    fn should_return_none_when_key_absent() {
        let map = make_map();
        assert_eq!(map.get_with_index(&1), None);
    }
}

mod iter {
    use super::*;

    #[test]
    fn should_return_empty_when_map_is_empty() {
        let map = make_map();
        assert_eq!(map.iter().count(), 0);
    }

    #[test]
    fn should_iterate_all_entries_in_ascending_index_order() {
        let mut map = make_map();
        map.insert(1, 30, entry(0));
        map.insert(2, 10, entry(1));
        map.insert(3, 20, entry(2));
        // Yields (index, key, value) ordered by index: 10, 20, 30
        let result: Vec<_> = map.iter().collect();
        assert_eq!(
            result,
            vec![(10, 2, entry(1)), (20, 3, entry(2)), (30, 1, entry(0))]
        );
    }

    #[test]
    fn should_include_entries_with_max_index() {
        let mut map = make_map();
        map.insert(1, u64::MAX, entry(99));
        map.insert(2, 0, entry(0));
        let result: Vec<_> = map.iter().collect();
        assert_eq!(result, vec![(0, 2, entry(0)), (u64::MAX, 1, entry(99))]);
    }
}

mod len_and_is_empty {
    use super::*;

    #[test]
    fn should_be_empty_when_new() {
        let map = make_map();
        assert!(map.is_empty());
        assert_eq!(map.len(), 0);
    }

    #[test]
    fn should_count_all_entries() {
        let mut map = make_map();
        map.insert(1, 100, entry(0));
        map.insert(2, 200, entry(1));
        assert_eq!(map.len(), 2);
        assert!(!map.is_empty());
    }
}
