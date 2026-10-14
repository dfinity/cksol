use crate::utils::timed_sized_map::{Entry, InsertError, TimedSizedMap};
use std::{num::NonZeroUsize, time::Duration};

#[test]
fn should_be_empty_on_creation() {
    let map: TimedSizedMap<&str, u32> = TimedSizedMap::new(Duration::from_nanos(10), capacity(4));

    assert!(map.is_empty());
    assert_eq!(map.len(), 0);
    assert_eq!(map.capacity(), capacity(4));
    assert_eq!(map.ttl(), Duration::from_nanos(10));
    assert_eq!(map.get(0, &"absent"), None);
}

#[test]
fn should_reject_a_new_key_when_full_of_live_entries() {
    let mut map = TimedSizedMap::new(Duration::from_nanos(1000), capacity(3));
    map.insert(1, "a", 1).unwrap();
    map.insert(2, "b", 2).unwrap();
    map.insert(3, "c", 3).unwrap();
    let before = map.clone();

    let rejected = map.insert(4, "d", 4);

    assert_eq!(
        rejected,
        Err(InsertError::AtCapacity { key: "d", value: 4 })
    );
    assert_eq!(map, before);
    assert_consistent(&map);
}

#[test]
fn should_reject_a_refresh_of_a_live_key() {
    let mut map = TimedSizedMap::new(Duration::from_nanos(1000), capacity(5));
    map.insert(1, "a", 1).unwrap();
    let before = map.clone();

    let rejected = map.insert(2, "a", 2);

    assert_eq!(
        rejected,
        Err(InsertError::AlreadyPresent { key: "a", value: 2 })
    );
    assert_eq!(map, before);
    assert_eq!(map.get(2, &"a"), Some(&1));
    assert_consistent(&map);
}

#[test]
fn should_not_evict_expired_entries_when_rejecting_a_live_refresh() {
    let mut map = TimedSizedMap::new(Duration::from_nanos(10), capacity(5));
    map.insert(0, "dormant", 1).unwrap();
    map.insert(5, "live", 2).unwrap();
    let before = map.clone();

    let rejected = map.insert(12, "live", 3);

    assert_eq!(
        rejected,
        Err(InsertError::AlreadyPresent {
            key: "live",
            value: 3
        })
    );
    assert_eq!(map, before);
    assert_eq!(map.len(), 2);
    assert_consistent(&map);
}

#[test]
fn should_admit_a_new_key_once_expired_entries_free_room() {
    let mut map = TimedSizedMap::new(Duration::from_nanos(10), capacity(2));
    map.insert(0, "a", 1).unwrap();
    map.insert(1, "b", 2).unwrap();

    let mut evicted = map.insert(12, "c", 3).unwrap();
    evicted.sort();

    assert_eq!(evicted, vec![("a", 1), ("b", 2)]);
    assert_eq!(map.len(), 1);
    assert_eq!(map.get(12, &"c"), Some(&3));
    assert_consistent(&map);
}

#[test]
fn should_readmit_an_expired_key_as_a_fresh_entry() {
    let mut map = TimedSizedMap::new(Duration::from_nanos(10), capacity(5));
    map.insert(0, "a", 1).unwrap();

    let evicted = map.insert(20, "a", 2).unwrap();

    assert_eq!(evicted, vec![("a", 1)]);
    assert_eq!(map.get(20, &"a"), Some(&2));
    assert_eq!(
        map.get_entry(20, &"a").map(|entry| entry.expires_at),
        Some(30)
    );
    assert_eq!(map.len(), 1);
    assert_consistent(&map);
}

#[test]
fn should_keep_an_entry_live_through_its_expiry_instant() {
    let mut map = TimedSizedMap::new(Duration::from_nanos(10), capacity(10));
    map.insert(0, "a", 1).unwrap();

    assert_eq!(map.get(10, &"a"), Some(&1));
    assert_eq!(map.get(11, &"a"), None);
    assert_eq!(map.get_value_mut(10, &"a"), Some(&mut 1));
    assert_eq!(map.get_value_mut(11, &"a"), None);
    assert_eq!(map.len(), 1);

    let evicted = map.evict_expired(11);

    assert_eq!(evicted, vec![("a", 1)]);
    assert!(map.is_empty());
    assert_consistent(&map);
}

#[test]
fn should_evict_nothing_before_the_earliest_expiry() {
    let mut map = TimedSizedMap::new(Duration::from_nanos(10), capacity(10));
    map.insert(0, "a", 1).unwrap();
    map.insert(5, "b", 2).unwrap();

    assert_eq!(map.evict_expired(10), vec![]);
    assert_eq!(map.evict_expired(11), vec![("a", 1)]);
    assert_eq!(map.evict_expired(15), vec![]);
    assert_eq!(map.evict_expired(16), vec![("b", 2)]);
    assert_consistent(&map);
}

#[test]
fn should_modify_a_live_value_in_place() {
    let mut map = TimedSizedMap::new(Duration::from_nanos(10), capacity(10));
    map.insert(0, "a", 1).unwrap();

    *map.get_value_mut(5, &"a").unwrap() = 7;

    assert_eq!(map.get(5, &"a"), Some(&7));
    assert_consistent(&map);
}

#[test]
fn should_clamp_the_expiry_of_an_inserted_entry_to_the_ttl() {
    let mut map = TimedSizedMap::new(Duration::from_nanos(10), capacity(5));

    map.insert_entry(
        0,
        "a",
        Entry {
            value: 1,
            expires_at: 100,
        },
    )
    .unwrap();

    assert_eq!(map.get(10, &"a"), Some(&1));
    assert_eq!(map.get(11, &"a"), None);
    assert_consistent(&map);
}

#[test]
fn should_keep_the_expiry_of_an_inserted_entry_shorter_than_the_ttl() {
    let mut map = TimedSizedMap::new(Duration::from_nanos(10), capacity(5));

    map.insert_entry(
        0,
        "a",
        Entry {
            value: 1,
            expires_at: 5,
        },
    )
    .unwrap();

    assert_eq!(map.get(5, &"a"), Some(&1));
    assert_eq!(map.get(6, &"a"), None);
    assert_consistent(&map);
}

#[test]
fn should_reject_an_already_expired_entry() {
    let mut map = TimedSizedMap::new(Duration::from_nanos(10), capacity(5));

    let rejected = map.insert_entry(
        20,
        "a",
        Entry {
            value: 1,
            expires_at: 19,
        },
    );

    assert_eq!(rejected, Err(InsertError::Expired { key: "a", value: 1 }));
    assert!(map.is_empty());
    assert_consistent(&map);
}

#[test]
fn should_reject_an_inserted_entry_when_full_of_live_entries() {
    let mut map = TimedSizedMap::new(Duration::from_nanos(1000), capacity(1));
    map.insert(1, "a", 1).unwrap();
    let before = map.clone();

    let rejected = map.insert_entry(
        2,
        "b",
        Entry {
            value: 2,
            expires_at: 500,
        },
    );

    assert_eq!(
        rejected,
        Err(InsertError::AtCapacity { key: "b", value: 2 })
    );
    assert_eq!(map, before);
    assert_consistent(&map);
}

#[test]
fn should_remove_an_entry_from_both_indices() {
    let mut map = TimedSizedMap::new(Duration::from_nanos(100), capacity(5));
    map.insert(0, "a", 1).unwrap();
    map.insert(0, "b", 2).unwrap();
    map.insert(10, "c", 3).unwrap();

    let removed = map.remove(&"a");

    assert_eq!(
        removed,
        Some(Entry {
            value: 1,
            expires_at: 100,
        })
    );
    assert_eq!(map.len(), 2);
    assert_eq!(map.get(0, &"a"), None);
    assert!(map.iter().all(|(key, _)| *key != "a"));
    assert!(map.iter_by_expiry().all(|(key, _)| *key != "a"));
    assert_consistent(&map);

    map.remove(&"b");

    assert_eq!(
        map.iter_by_expiry()
            .map(|(key, _)| *key)
            .collect::<Vec<_>>(),
        vec!["c"]
    );
    assert_consistent(&map);
}

#[test]
fn should_remove_an_expired_entry() {
    let mut map = TimedSizedMap::new(Duration::from_nanos(10), capacity(5));
    map.insert(0, "a", 1).unwrap();

    let removed = map.remove(&"a");

    assert_eq!(removed.map(|entry| entry.value), Some(1));
    assert!(map.is_empty());
    assert_consistent(&map);
}

#[test]
fn should_return_none_when_removing_an_absent_key() {
    let mut map = TimedSizedMap::new(Duration::from_nanos(100), capacity(5));
    map.insert(0, "a", 1).unwrap();
    let before = map.clone();

    assert_eq!(map.remove(&"absent"), None);
    assert_eq!(map, before);
    assert_consistent(&map);
}

#[test]
fn should_keep_the_indices_consistent_through_churn() {
    let mut map = TimedSizedMap::new(Duration::from_nanos(100), capacity(4));
    for (index, key) in ["a", "b", "c", "d"].into_iter().enumerate() {
        map.insert(index as u64 + 1, key, index).unwrap();
    }
    assert_eq!(map.len(), 4);
    assert_consistent(&map);

    assert!(matches!(
        map.insert(5, "e", 4),
        Err(InsertError::AtCapacity { .. })
    ));
    assert!(matches!(
        map.insert(6, "a", 99),
        Err(InsertError::AlreadyPresent { .. })
    ));
    assert_eq!(map.len(), 4);
    assert_consistent(&map);

    let evicted = map.evict_expired(1000);

    assert_eq!(evicted.len(), 4);
    assert!(map.is_empty());
    assert_consistent(&map);
}

#[test]
fn should_saturate_the_expiry_of_a_ttl_beyond_the_epoch_range() {
    let mut map = TimedSizedMap::new(Duration::MAX, capacity(5));

    map.insert(u64::MAX - 1, "a", 1).unwrap();

    assert_eq!(
        map.get_entry(u64::MAX, &"a").map(|entry| entry.expires_at),
        Some(u64::MAX)
    );
}

#[test]
fn should_round_trip_through_from_ordered_entries() {
    let mut map = TimedSizedMap::new(Duration::from_nanos(100), capacity(5));
    map.insert(0, "b", 2).unwrap();
    map.insert(0, "a", 1).unwrap();
    map.insert(10, "c", 3).unwrap();

    let entries: Vec<_> = map
        .iter_by_expiry()
        .map(|(key, entry)| (*key, entry.clone()))
        .collect();
    let restored = TimedSizedMap::from_ordered_entries(map.ttl(), map.capacity(), entries);

    assert_eq!(restored, map);
    assert_consistent(&restored);
}

#[test]
fn should_preserve_expired_entries_when_rebuilding() {
    let mut map = TimedSizedMap::new(Duration::from_nanos(10), capacity(5));
    map.insert(0, "a", 1).unwrap();
    let entries: Vec<_> = map
        .iter_by_expiry()
        .map(|(key, entry)| (*key, entry.clone()))
        .collect();

    let restored = TimedSizedMap::from_ordered_entries(map.ttl(), map.capacity(), entries);

    assert_eq!(restored, map);
    assert_eq!(restored.len(), 1);
    assert_eq!(restored.get(20, &"a"), None);
    assert_consistent(&restored);
}

#[test]
fn should_rebuild_beyond_the_capacity_without_eviction() {
    let entries = (0..3).map(|index| {
        (
            index,
            Entry {
                value: index,
                expires_at: 10,
            },
        )
    });

    let restored =
        TimedSizedMap::from_ordered_entries(Duration::from_nanos(1), capacity(1), entries);

    assert_eq!(restored.len(), 3);
    assert_eq!(
        restored
            .iter_by_expiry()
            .map(|(key, _)| *key)
            .collect::<Vec<_>>(),
        vec![0, 1, 2]
    );
    assert_consistent(&restored);
}

#[test]
#[should_panic(expected = "duplicate key")]
fn should_panic_on_a_duplicate_key_in_from_ordered_entries() {
    let entry = Entry {
        value: 1,
        expires_at: 10,
    };

    let _ = TimedSizedMap::from_ordered_entries(
        Duration::from_nanos(10),
        capacity(5),
        vec![("a", entry.clone()), ("a", entry)],
    );
}

fn capacity(n: usize) -> NonZeroUsize {
    NonZeroUsize::new(n).unwrap()
}

fn assert_consistent<K: Ord + Clone + std::fmt::Debug, V: PartialEq + std::fmt::Debug>(
    map: &TimedSizedMap<K, V>,
) {
    let indexed: Vec<&K> = map.iter_by_expiry().map(|(key, _)| key).collect();
    assert_eq!(indexed.len(), map.len(), "index and entry counts disagree");
    for (key, entry) in map.iter() {
        let indexed_entry = map
            .iter_by_expiry()
            .find(|(indexed_key, _)| *indexed_key == key)
            .map(|(_, entry)| entry)
            .unwrap_or_else(|| panic!("entry {key:?} missing from the expiry index"));
        assert_eq!(indexed_entry, entry);
    }
    let expiries: Vec<u64> = map
        .iter_by_expiry()
        .map(|(_, entry)| entry.expires_at)
        .collect();
    assert!(
        expiries.is_sorted(),
        "expiry index out of order: {expiries:?}"
    );
}
