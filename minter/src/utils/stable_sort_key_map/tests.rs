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
    fn should_store_value_in_by_key() {
        let mut map = make_map();
        map.insert(1, 100, entry(42));
        assert_eq!(map.get(&1), Some(entry(42)));
        assert_eq!(map.len(), 1);
    }

    #[test]
    fn should_index_entry() {
        let mut map = make_map();
        map.insert(1, 50, entry(0));
        assert_eq!(map.peek(), Some((50, 1)));
    }

    #[test]
    fn should_update_index_when_index_changes() {
        let mut map = make_map();
        map.insert(1, 100, entry(0));
        map.insert(1, 200, entry(1));
        assert_eq!(map.peek(), Some((200, 1)));
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

mod peek {
    use super::*;

    #[test]
    fn should_return_none_when_empty() {
        let map = make_map();
        assert_eq!(map.peek(), None);
    }

    #[test]
    fn should_return_smallest_index() {
        let mut map = make_map();
        map.insert(1, 300, entry(0));
        map.insert(2, 100, entry(1));
        map.insert(3, 200, entry(2));
        assert_eq!(map.peek(), Some((100, 2)));
    }
}

mod iter_by_index_up_to {
    use super::*;

    #[test]
    fn should_return_empty_when_nothing_in_range() {
        let mut map = make_map();
        map.insert(1, 100, entry(0));
        map.insert(2, 200, entry(1));
        let result: Vec<_> = map.iter_by_index_up_to(&50).collect();
        assert!(result.is_empty())
    }

    #[test]
    fn should_return_entries_with_index_at_or_below_max() {
        let mut map = make_map();
        map.insert(1, 10, entry(0));
        map.insert(2, 20, entry(1));
        map.insert(3, 30, entry(2));
        let result: Vec<(u64, _)> = map.iter_by_index_up_to(&20).collect();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].0, 1);
        assert_eq!(result[1].0, 2);
    }

    #[test]
    fn should_iterate_in_ascending_index_order() {
        let mut map = make_map();
        map.insert(1, 30, entry(0));
        map.insert(2, 10, entry(1));
        map.insert(3, 20, entry(2));
        let keys: Vec<u64> = map.iter_by_index_up_to(&u64::MAX).map(|(k, _)| k).collect();
        assert_eq!(keys, vec![2, 3, 1]); // ordered by index: 10, 20, 30
    }

    #[test]
    fn should_stop_at_first_entry_exceeding_max() {
        let mut map = make_map();
        map.insert(1, 10, entry(0));
        map.insert(2, 20, entry(1));
        map.insert(3, 30, entry(2));
        map.insert(4, 40, entry(3));
        let result: Vec<_> = map.iter_by_index_up_to(&30).collect();
        assert_eq!(result.len(), 3);
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
