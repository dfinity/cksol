use super::InsertionOrderedMap;

mod insert {
    use super::*;

    #[test]
    fn should_insert_new_entry() {
        let mut map: InsertionOrderedMap<u32, &str> = InsertionOrderedMap::new();
        assert_eq!(map.insert(1, "a"), None);
        assert_eq!(map.len(), 1);
        assert_eq!(map.get(&1), Some(&"a"));
    }

    #[test]
    fn should_return_old_value_on_reinsertion() {
        let mut map = InsertionOrderedMap::new();
        map.insert(1u32, "a");
        assert_eq!(map.insert(1, "b"), Some("a"));
        assert_eq!(map.get(&1), Some(&"b"));
        assert_eq!(map.len(), 1);
    }

    #[test]
    fn should_move_reinserted_key_to_end() {
        let mut map = InsertionOrderedMap::new();
        map.insert(1u32, "a");
        map.insert(2, "b");
        map.insert(1, "a2"); // re-insert key 1

        let order: Vec<_> = map.iter().map(|(k, _)| *k).collect();
        assert_eq!(order, vec![2, 1]);
    }
}

mod remove {
    use super::*;

    #[test]
    fn should_remove_existing_key() {
        let mut map = InsertionOrderedMap::new();
        map.insert(1u32, "a");
        assert_eq!(map.remove(&1), Some("a"));
        assert!(map.is_empty());
    }

    #[test]
    fn should_return_none_for_absent_key() {
        let mut map: InsertionOrderedMap<u32, &str> = InsertionOrderedMap::new();
        assert_eq!(map.remove(&99), None);
    }

    #[test]
    fn should_preserve_order_of_remaining_keys() {
        let mut map = InsertionOrderedMap::new();
        map.insert(1u32, "a");
        map.insert(2, "b");
        map.insert(3, "c");
        map.remove(&2);

        let order: Vec<_> = map.iter().map(|(k, _)| *k).collect();
        assert_eq!(order, vec![1, 3]);
    }
}

mod get {
    use super::*;

    #[test]
    fn should_return_value_for_present_key() {
        let mut map = InsertionOrderedMap::new();
        map.insert(42u32, "hello");
        assert_eq!(map.get(&42), Some(&"hello"));
    }

    #[test]
    fn should_return_none_for_absent_key() {
        let map: InsertionOrderedMap<u32, &str> = InsertionOrderedMap::new();
        assert_eq!(map.get(&1), None);
    }
}

mod contains_key {
    use super::*;

    #[test]
    fn should_return_true_for_present_key() {
        let mut map = InsertionOrderedMap::new();
        map.insert(1u32, "a");
        assert!(map.contains_key(&1));
    }

    #[test]
    fn should_return_false_for_absent_key() {
        let map: InsertionOrderedMap<u32, &str> = InsertionOrderedMap::new();
        assert!(!map.contains_key(&1));
    }
}

mod len_and_is_empty {
    use super::*;

    #[test]
    fn should_be_empty_when_new() {
        let map: InsertionOrderedMap<u32, u32> = InsertionOrderedMap::new();
        assert!(map.is_empty());
        assert_eq!(map.len(), 0);
    }

    #[test]
    fn should_track_len_through_inserts_and_removes() {
        let mut map = InsertionOrderedMap::new();
        map.insert(1u32, "a");
        map.insert(2, "b");
        assert_eq!(map.len(), 2);
        map.remove(&1);
        assert_eq!(map.len(), 1);
        assert!(!map.is_empty());
    }
}

mod iter {
    use super::*;

    #[test]
    fn should_iterate_in_insertion_order() {
        let mut map = InsertionOrderedMap::new();
        map.insert(3u32, "c");
        map.insert(1, "a");
        map.insert(2, "b");

        let pairs: Vec<_> = map.iter().map(|(k, v)| (*k, *v)).collect();
        assert_eq!(pairs, vec![(3, "c"), (1, "a"), (2, "b")]);
    }

    #[test]
    fn should_iterate_in_reverse_via_rev() {
        let mut map = InsertionOrderedMap::new();
        map.insert(1u32, "a");
        map.insert(2, "b");
        map.insert(3, "c");

        let keys: Vec<_> = map.iter().rev().map(|(k, _)| *k).collect();
        assert_eq!(keys, vec![3, 2, 1]);
    }

    #[test]
    fn should_work_in_for_loop() {
        let mut map = InsertionOrderedMap::new();
        map.insert(10u32, 100u32);
        map.insert(20, 200);

        let mut sum = 0;
        for (_, v) in &map {
            sum += v;
        }
        assert_eq!(sum, 300);
    }
}

mod keys {
    use super::*;

    #[test]
    fn should_yield_keys_in_insertion_order() {
        let mut map = InsertionOrderedMap::new();
        map.insert(3u32, ());
        map.insert(1, ());
        map.insert(2, ());

        let keys: Vec<_> = map.keys().copied().collect();
        assert_eq!(keys, vec![3, 1, 2]);
    }
}

mod values {
    use super::*;

    #[test]
    fn should_yield_values_in_insertion_order() {
        let mut map = InsertionOrderedMap::new();
        map.insert("c", 3u32);
        map.insert("a", 1);
        map.insert("b", 2);

        let values: Vec<_> = map.values().copied().collect();
        assert_eq!(values, vec![3, 1, 2]);
    }
}

mod values_mut {
    use super::*;

    #[test]
    fn should_allow_in_place_mutation() {
        let mut map = InsertionOrderedMap::new();
        map.insert(1u32, 10u32);
        map.insert(2, 20);

        for v in map.values_mut() {
            *v *= 2;
        }

        assert_eq!(map.get(&1), Some(&20));
        assert_eq!(map.get(&2), Some(&40));
    }
}

mod partial_eq {
    use super::*;

    #[test]
    fn should_be_equal_with_same_entries_in_same_order() {
        let mut a = InsertionOrderedMap::new();
        a.insert(1u32, "x");
        a.insert(2, "y");

        let mut b = InsertionOrderedMap::new();
        b.insert(1u32, "x");
        b.insert(2, "y");

        assert_eq!(a, b);
    }

    #[test]
    fn should_not_be_equal_with_different_values() {
        let mut a = InsertionOrderedMap::new();
        a.insert(1u32, "x");

        let mut b = InsertionOrderedMap::new();
        b.insert(1u32, "z");

        assert_ne!(a, b);
    }

    #[test]
    fn should_be_equal_regardless_of_internal_indices() {
        // Map `a`: key 1 gets seq=0, key 2 gets seq=1.
        let mut a = InsertionOrderedMap::new();
        a.insert(1u32, "x");
        a.insert(2u32, "y");

        // Map `b`: key 1 gets seq=0, key 3 gets seq=1 (then removed),
        // key 2 gets seq=2 — a different internal index than in `a`.
        // The visible entries and their order are identical.
        let mut b = InsertionOrderedMap::new();
        b.insert(1u32, "x");
        b.insert(3u32, "z");
        b.remove(&3u32);
        b.insert(2u32, "y");

        assert_eq!(a, b);
    }
}
