use super::{InsertionOrderedMap, InsertionOrderedSet};

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

    #[test]
    fn should_support_rev() {
        let mut map = InsertionOrderedMap::new();
        map.insert(1u32, ());
        map.insert(2, ());
        map.insert(3, ());

        let keys: Vec<_> = map.keys().rev().copied().collect();
        assert_eq!(keys, vec![3, 2, 1]);
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

mod extract_if {
    use super::*;

    #[test]
    fn should_remove_and_return_matching_entries() {
        let mut map = InsertionOrderedMap::new();
        map.insert(1u32, 10u32);
        map.insert(2, 20);
        map.insert(3, 30);

        let extracted: Vec<_> = map.extract_if(|_, v| *v > 15);
        assert_eq!(extracted, vec![(2, 20), (3, 30)]);
        assert_eq!(map.len(), 1);
        assert_eq!(map.get(&1), Some(&10));
    }

    #[test]
    fn should_return_empty_when_none_match() {
        let mut map = InsertionOrderedMap::new();
        map.insert(1u32, 1u32);
        let extracted = map.extract_if(|_, v| *v > 100);
        assert!(extracted.is_empty());
        assert_eq!(map.len(), 1);
    }
}

mod partial_eq {
    use super::*;

    #[test]
    fn should_be_equal_with_same_entries() {
        let mut a = InsertionOrderedMap::new();
        a.insert(1u32, "x");
        a.insert(2, "y");

        let mut b = InsertionOrderedMap::new();
        b.insert(1u32, "x");
        b.insert(2, "y");

        assert_eq!(a, b);
    }

    #[test]
    fn should_be_equal_regardless_of_insertion_order() {
        let mut a = InsertionOrderedMap::new();
        a.insert(1u32, "x");
        a.insert(2, "y");

        let mut b = InsertionOrderedMap::new();
        b.insert(2u32, "y");
        b.insert(1, "x");

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
}

// ── InsertionOrderedSet ─────────────────────────────────────────────────────

mod set_insert {
    use super::*;

    #[test]
    fn should_return_true_for_new_key() {
        let mut set = InsertionOrderedSet::new();
        assert!(set.insert(1u32));
    }

    #[test]
    fn should_return_false_for_duplicate_key() {
        let mut set = InsertionOrderedSet::new();
        set.insert(1u32);
        assert!(!set.insert(1));
    }
}

mod set_remove {
    use super::*;

    #[test]
    fn should_return_true_when_present() {
        let mut set = InsertionOrderedSet::new();
        set.insert(1u32);
        assert!(set.remove(&1));
        assert!(!set.contains(&1));
    }

    #[test]
    fn should_return_false_when_absent() {
        let mut set: InsertionOrderedSet<u32> = InsertionOrderedSet::new();
        assert!(!set.remove(&99));
    }
}

mod set_contains {
    use super::*;

    #[test]
    fn should_return_true_for_inserted_key() {
        let mut set = InsertionOrderedSet::new();
        set.insert(42u32);
        assert!(set.contains(&42));
    }

    #[test]
    fn should_return_false_for_absent_key() {
        let set: InsertionOrderedSet<u32> = InsertionOrderedSet::new();
        assert!(!set.contains(&1));
    }
}

mod set_iter {
    use super::*;

    #[test]
    fn should_yield_keys_in_insertion_order() {
        let mut set = InsertionOrderedSet::new();
        set.insert(3u32);
        set.insert(1);
        set.insert(2);

        let keys: Vec<_> = set.iter().copied().collect();
        assert_eq!(keys, vec![3, 1, 2]);
    }

    #[test]
    fn should_support_rev() {
        let mut set = InsertionOrderedSet::new();
        set.insert(1u32);
        set.insert(2);
        set.insert(3);

        let keys: Vec<_> = set.iter().rev().copied().collect();
        assert_eq!(keys, vec![3, 2, 1]);
    }
}
