use super::SortedKeyMap;

mod insert {
    use super::*;

    #[test]
    fn should_insert_new_entry() {
        let mut map: SortedKeyMap<u32, u64, &str> = SortedKeyMap::new();
        assert_eq!(map.insert(1, 10, "a"), None);
        assert_eq!(map.len(), 1);
        assert_eq!(map.get(&1), Some(&"a"));
    }

    #[test]
    fn should_return_old_value_on_reinsertion() {
        let mut map: SortedKeyMap<u32, u64, &str> = SortedKeyMap::new();
        map.insert(1, 10, "a");
        assert_eq!(map.insert(1, 20, "b"), Some("a"));
        assert_eq!(map.get(&1), Some(&"b"));
        assert_eq!(map.len(), 1);
    }

    #[test]
    fn should_update_index_on_reinsertion() {
        let mut map: SortedKeyMap<u32, u64, &str> = SortedKeyMap::new();
        map.insert(1, 10, "a");
        map.insert(2, 20, "b");
        map.insert(1, 30, "a2"); // re-insert with new index

        let triples: Vec<_> = map.iter().map(|(i, k, v)| (*i, *k, *v)).collect();
        assert_eq!(triples, vec![(20, 2, "b"), (30, 1, "a2")]);
    }
}

mod remove {
    use super::*;

    #[test]
    fn should_remove_existing_key() {
        let mut map: SortedKeyMap<u32, u64, &str> = SortedKeyMap::new();
        map.insert(1, 10, "a");
        assert_eq!(map.remove(&1), Some("a"));
        assert!(map.is_empty());
    }

    #[test]
    fn should_return_none_for_absent_key() {
        let mut map: SortedKeyMap<u32, u64, &str> = SortedKeyMap::new();
        assert_eq!(map.remove(&99), None);
    }

    #[test]
    fn should_preserve_order_of_remaining_entries() {
        let mut map: SortedKeyMap<u32, u64, &str> = SortedKeyMap::new();
        map.insert(1, 10, "a");
        map.insert(2, 20, "b");
        map.insert(3, 30, "c");
        map.remove(&2);

        let keys: Vec<_> = map.iter().map(|(_, k, _)| *k).collect();
        assert_eq!(keys, vec![1, 3]);
    }
}

mod get {
    use super::*;

    #[test]
    fn should_return_value_for_present_key() {
        let mut map: SortedKeyMap<u32, u64, &str> = SortedKeyMap::new();
        map.insert(42, 0, "hello");
        assert_eq!(map.get(&42), Some(&"hello"));
    }

    #[test]
    fn should_return_none_for_absent_key() {
        let map: SortedKeyMap<u32, u64, &str> = SortedKeyMap::new();
        assert_eq!(map.get(&1), None);
    }
}

mod get_with_index {
    use super::*;

    #[test]
    fn should_return_index_and_value() {
        let mut map: SortedKeyMap<u32, u64, &str> = SortedKeyMap::new();
        map.insert(1, 42, "foo");
        assert_eq!(map.get_with_index(&1), Some((&42u64, &"foo")));
    }

    #[test]
    fn should_return_none_for_absent_key() {
        let map: SortedKeyMap<u32, u64, &str> = SortedKeyMap::new();
        assert_eq!(map.get_with_index(&1), None);
    }
}

mod contains_key {
    use super::*;

    #[test]
    fn should_return_true_for_present_key() {
        let mut map: SortedKeyMap<u32, u64, &str> = SortedKeyMap::new();
        map.insert(1, 0, "a");
        assert!(map.contains_key(&1));
    }

    #[test]
    fn should_return_false_for_absent_key() {
        let map: SortedKeyMap<u32, u64, &str> = SortedKeyMap::new();
        assert!(!map.contains_key(&1));
    }
}

mod iter {
    use super::*;

    #[test]
    fn should_iterate_in_ascending_index_order() {
        let mut map: SortedKeyMap<u32, u64, &str> = SortedKeyMap::new();
        map.insert(3, 30, "c");
        map.insert(1, 10, "a");
        map.insert(2, 20, "b");

        let triples: Vec<_> = map.iter().map(|(i, k, v)| (*i, *k, *v)).collect();
        assert_eq!(triples, vec![(10, 1, "a"), (20, 2, "b"), (30, 3, "c")]);
    }

    #[test]
    fn should_order_equal_indices_by_key() {
        let mut map: SortedKeyMap<u32, u64, &str> = SortedKeyMap::new();
        map.insert(2, 10, "b");
        map.insert(1, 10, "a"); // same index as key 2

        let triples: Vec<_> = map.iter().map(|(i, k, v)| (*i, *k, *v)).collect();
        assert_eq!(triples, vec![(10, 1, "a"), (10, 2, "b")]);
    }

    #[test]
    fn should_support_reverse_iteration() {
        let mut map: SortedKeyMap<u32, u64, &str> = SortedKeyMap::new();
        map.insert(1, 10, "a");
        map.insert(2, 20, "b");
        map.insert(3, 30, "c");

        let keys: Vec<_> = map.iter().rev().map(|(_, k, _)| *k).collect();
        assert_eq!(keys, vec![3, 2, 1]);
    }
}

mod len_and_is_empty {
    use super::*;

    #[test]
    fn should_be_empty_when_new() {
        let map: SortedKeyMap<u32, u64, u32> = SortedKeyMap::new();
        assert!(map.is_empty());
        assert_eq!(map.len(), 0);
    }

    #[test]
    fn should_track_len_through_inserts_and_removes() {
        let mut map: SortedKeyMap<u32, u64, &str> = SortedKeyMap::new();
        map.insert(1, 10, "a");
        map.insert(2, 20, "b");
        assert_eq!(map.len(), 2);
        map.remove(&1);
        assert_eq!(map.len(), 1);
        assert!(!map.is_empty());
    }
}
