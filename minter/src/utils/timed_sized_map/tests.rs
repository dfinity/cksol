use crate::utils::timed_sized_map::{Entry, InsertError, TimedSizedMap, Timestamp};
use std::{num::NonZeroUsize, time::Duration};

#[test]
fn should_be_empty_on_creation() {
    let map: TimedSizedMap<&str, u32> = TimedSizedMap::new(Duration::from_nanos(10), capacity(4));

    assert!(map.is_empty());
    assert_eq!(map.len(), 0);
    assert_eq!(map.capacity(), capacity(4));
    assert_eq!(map.ttl(), Duration::from_nanos(10));
    assert_eq!(map.get(at(0), &"absent"), None);
}

#[test]
fn should_reject_a_new_key_when_full_of_live_entries() {
    let mut map = TimedSizedMap::new(Duration::from_nanos(1000), capacity(3));
    map.insert(at(1), "a", 1).unwrap();
    map.insert(at(2), "b", 2).unwrap();
    map.insert(at(3), "c", 3).unwrap();
    let before = map.clone();

    let rejected = map.insert(at(4), "d", 4);

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
    map.insert(at(1), "a", 1).unwrap();
    let before = map.clone();

    let rejected = map.insert(at(2), "a", 2);

    assert_eq!(
        rejected,
        Err(InsertError::AlreadyPresent { key: "a", value: 2 })
    );
    assert_eq!(map, before);
    assert_eq!(map.get(at(2), &"a"), Some(&1));
    assert_consistent(&map);
}

#[test]
fn should_not_evict_expired_entries_when_rejecting_a_live_refresh() {
    let mut map = TimedSizedMap::new(Duration::from_nanos(10), capacity(5));
    map.insert(at(0), "dormant", 1).unwrap();
    map.insert(at(5), "live", 2).unwrap();
    let before = map.clone();

    let rejected = map.insert(at(12), "live", 3);

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
    map.insert(at(0), "a", 1).unwrap();
    map.insert(at(1), "b", 2).unwrap();

    let mut evicted = map.insert(at(12), "c", 3).unwrap();
    evicted.sort();

    assert_eq!(evicted, vec![("a", 1), ("b", 2)]);
    assert_eq!(map.len(), 1);
    assert_eq!(map.get(at(12), &"c"), Some(&3));
    assert_consistent(&map);
}

#[test]
fn should_readmit_an_expired_key_as_a_fresh_entry() {
    // A capacity of 1 also checks that the expired entry is evicted before the capacity
    // check, since it is the one filling the map.
    for cap in [capacity(5), capacity(1)] {
        let mut map = TimedSizedMap::new(Duration::from_nanos(10), cap);
        map.insert(at(0), "a", 1).unwrap();

        let evicted = map.insert(at(20), "a", 2).unwrap();

        assert_eq!(evicted, vec![("a", 1)]);
        assert_eq!(map.get(at(20), &"a"), Some(&2));
        assert_eq!(
            map.get_entry(at(20), &"a").map(|entry| entry.expires_at),
            Some(at(30))
        );
        assert_eq!(map.len(), 1);
        assert_consistent(&map);
    }
}

#[test]
fn should_keep_an_entry_live_through_its_expiry_instant() {
    let mut map = TimedSizedMap::new(Duration::from_nanos(10), capacity(10));
    map.insert(at(0), "a", 1).unwrap();

    assert_eq!(map.get(at(10), &"a"), Some(&1));
    assert_eq!(map.get(at(11), &"a"), None);
    assert_eq!(map.get_value_mut(at(10), &"a"), Some(&mut 1));
    assert_eq!(map.get_value_mut(at(11), &"a"), None);
    assert_eq!(map.len(), 1);

    let evicted = map.evict_expired(at(11));

    assert_eq!(evicted, vec![("a", 1)]);
    assert!(map.is_empty());
    assert_consistent(&map);
}

#[test]
fn should_evict_nothing_before_the_earliest_expiry() {
    let mut map = TimedSizedMap::new(Duration::from_nanos(10), capacity(10));
    map.insert(at(0), "a", 1).unwrap();
    map.insert(at(5), "b", 2).unwrap();

    assert_eq!(map.evict_expired(at(10)), vec![]);
    assert_eq!(map.evict_expired(at(11)), vec![("a", 1)]);
    assert_eq!(map.evict_expired(at(15)), vec![]);
    assert_eq!(map.evict_expired(at(16)), vec![("b", 2)]);
    assert_consistent(&map);
}

#[test]
fn should_modify_a_live_value_in_place() {
    let mut map = TimedSizedMap::new(Duration::from_nanos(10), capacity(10));
    map.insert(at(0), "a", 1).unwrap();

    *map.get_value_mut(at(5), &"a").unwrap() = 7;

    assert_eq!(map.get(at(5), &"a"), Some(&7));
    assert_consistent(&map);
}

#[test]
fn should_clamp_the_expiry_of_an_inserted_entry_to_the_ttl() {
    let mut map = TimedSizedMap::new(Duration::from_nanos(10), capacity(5));

    map.insert_entry(
        at(0),
        "a",
        Entry {
            value: 1,
            expires_at: at(100),
        },
    )
    .unwrap();

    assert_eq!(map.get(at(10), &"a"), Some(&1));
    assert_eq!(map.get(at(11), &"a"), None);
    assert_consistent(&map);
}

#[test]
fn should_keep_the_expiry_of_an_inserted_entry_shorter_than_the_ttl() {
    let mut map = TimedSizedMap::new(Duration::from_nanos(10), capacity(5));

    map.insert_entry(
        at(0),
        "a",
        Entry {
            value: 1,
            expires_at: at(5),
        },
    )
    .unwrap();

    assert_eq!(map.get(at(5), &"a"), Some(&1));
    assert_eq!(map.get(at(6), &"a"), None);
    assert_consistent(&map);
}

#[test]
fn should_ignore_an_already_expired_entry() {
    let mut map = TimedSizedMap::new(Duration::from_nanos(10), capacity(5));
    // An expired entry that an insertion would evict shows that the no-op leaves the map as
    // it is.
    map.insert(at(0), "x", 0).unwrap();
    let before = map.clone();

    let evicted = map
        .insert_entry(
            at(20),
            "a",
            Entry {
                value: 1,
                expires_at: at(19),
            },
        )
        .unwrap();

    assert!(evicted.is_empty());
    assert_eq!(map, before);
    assert_eq!(map.get(at(20), &"a"), None);
    assert_consistent(&map);
}

#[test]
fn should_reject_an_inserted_entry_when_full_of_live_entries() {
    let mut map = TimedSizedMap::new(Duration::from_nanos(1000), capacity(1));
    map.insert(at(1), "a", 1).unwrap();
    let before = map.clone();

    let rejected = map.insert_entry(
        at(2),
        "b",
        Entry {
            value: 2,
            expires_at: at(500),
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
    map.insert(at(0), "a", 1).unwrap();
    map.insert(at(0), "b", 2).unwrap();
    map.insert(at(10), "c", 3).unwrap();

    let removed = map.remove(&"a");

    assert_eq!(
        removed,
        Some(Entry {
            value: 1,
            expires_at: at(100),
        })
    );
    assert_eq!(map.len(), 2);
    assert_eq!(map.get(at(0), &"a"), None);
    assert!(map.iter().all(|(key, _)| *key != "a"));
    assert!(map.iter_by_expiry().all(|(key, _)| *key != "a"));
    assert_consistent(&map);

    // "c" sits in a later bucket than "b", so its removal must find its own bucket.
    map.remove(&"c");

    assert_eq!(
        map.iter_by_expiry()
            .map(|(key, _)| *key)
            .collect::<Vec<_>>(),
        vec!["b"]
    );
    assert_consistent(&map);

    map.remove(&"b");

    assert!(map.is_empty());
    assert_consistent(&map);
}

#[test]
fn should_remove_an_expired_entry() {
    let mut map = TimedSizedMap::new(Duration::from_nanos(10), capacity(5));
    map.insert(at(0), "a", 1).unwrap();

    let removed = map.remove(&"a");

    assert_eq!(removed.map(|entry| entry.value), Some(1));
    assert!(map.is_empty());
    assert_consistent(&map);
}

#[test]
fn should_return_none_when_removing_an_absent_key() {
    let mut map = TimedSizedMap::new(Duration::from_nanos(100), capacity(5));
    map.insert(at(0), "a", 1).unwrap();
    let before = map.clone();

    assert_eq!(map.remove(&"absent"), None);
    assert_eq!(map, before);
    assert_consistent(&map);
}

#[test]
fn should_keep_the_indices_consistent_through_churn() {
    let mut map = TimedSizedMap::new(Duration::from_nanos(100), capacity(4));
    for (index, key) in ["a", "b", "c", "d"].into_iter().enumerate() {
        map.insert(at(index as u64 + 1), key, index).unwrap();
    }
    assert_eq!(map.len(), 4);
    assert_consistent(&map);

    assert!(matches!(
        map.insert(at(5), "e", 4),
        Err(InsertError::AtCapacity { .. })
    ));
    assert!(matches!(
        map.insert(at(6), "a", 99),
        Err(InsertError::AlreadyPresent { .. })
    ));
    assert_eq!(map.len(), 4);
    assert_consistent(&map);

    let evicted = map.evict_expired(at(1000));

    assert_eq!(evicted.len(), 4);
    assert!(map.is_empty());
    assert_consistent(&map);
}

#[test]
fn should_saturate_the_expiry_of_a_ttl_beyond_the_epoch_range() {
    // A ttl whose nanoseconds exceed u64::MAX pins the conversion of the ttl.
    let mut map = TimedSizedMap::new(Duration::from_secs(u64::MAX), capacity(5));

    map.insert(at(0), "a", 1).unwrap();

    assert_eq!(
        map.get_entry(at(0), &"a").map(|entry| entry.expires_at),
        Some(at(u64::MAX))
    );

    // An insertion close to the end of the epoch range pins the addition of the ttl.
    let mut map = TimedSizedMap::new(Duration::from_nanos(10), capacity(5));

    map.insert(at(u64::MAX - 1), "a", 1).unwrap();

    assert_eq!(
        map.get_entry(at(u64::MAX), &"a")
            .map(|entry| entry.expires_at),
        Some(at(u64::MAX))
    );
}

#[test]
fn should_round_trip_through_from_ordered_entries() {
    let mut map = TimedSizedMap::new(Duration::from_nanos(100), capacity(5));
    map.insert(at(0), "b", 2).unwrap();
    map.insert(at(0), "a", 1).unwrap();
    map.insert(at(10), "c", 3).unwrap();

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
    map.insert(at(0), "a", 1).unwrap();
    let entries: Vec<_> = map
        .iter_by_expiry()
        .map(|(key, entry)| (*key, entry.clone()))
        .collect();

    let restored = TimedSizedMap::from_ordered_entries(map.ttl(), map.capacity(), entries);

    assert_eq!(restored, map);
    assert_eq!(restored.len(), 1);
    assert_eq!(restored.get(at(20), &"a"), None);
    assert_consistent(&restored);
}

#[test]
fn should_rebuild_beyond_the_capacity_without_eviction() {
    let entries = (0..3).map(|index| {
        (
            index,
            Entry {
                value: index,
                expires_at: at(10),
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
        expires_at: at(10),
    };

    let _ = TimedSizedMap::from_ordered_entries(
        Duration::from_nanos(10),
        capacity(5),
        vec![("a", entry.clone()), ("a", entry)],
    );
}

fn at(nanos: u64) -> Timestamp {
    Timestamp::from_nanos(nanos)
}

fn capacity(n: usize) -> NonZeroUsize {
    NonZeroUsize::new(n).unwrap()
}

fn assert_consistent<K: Ord + Clone + std::fmt::Debug, V>(map: &TimedSizedMap<K, V>) {
    let indexed: Vec<&K> = map.by_expiry.values().flatten().collect();
    assert_eq!(
        indexed.len(),
        map.entries.len(),
        "index and entry counts disagree"
    );
    for (key, entry) in &map.entries {
        let bucket = map
            .by_expiry
            .get(&entry.expires_at)
            .unwrap_or_else(|| panic!("expiry of entry {key:?} missing from the expiry index"));
        assert!(
            bucket.contains(key),
            "entry {key:?} missing from the bucket of its expiry"
        );
    }
    assert!(
        map.by_expiry.values().all(|bucket| !bucket.is_empty()),
        "empty bucket left in the expiry index"
    );
}
