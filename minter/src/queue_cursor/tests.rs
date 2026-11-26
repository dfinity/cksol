use super::QueueCursor;
use std::collections::{BTreeMap, BTreeSet};

fn queue(keys: impl IntoIterator<Item = u64>) -> BTreeMap<u64, ()> {
    keys.into_iter().map(|key| (key, ())).collect()
}

fn keys(cursor: &QueueCursor<u64>, queue: &BTreeMap<u64, ()>, limit: usize) -> Vec<u64> {
    cursor
        .round(queue, limit)
        .into_iter()
        .map(|(key, _)| *key)
        .collect()
}

#[test]
fn should_start_at_the_first_entry_and_respect_the_limit() {
    let queue = queue([1, 2, 3]);
    let cursor = QueueCursor::new();
    assert_eq!(keys(&cursor, &queue, 2), [1, 2]);
    assert_eq!(keys(&cursor, &queue, 10), [1, 2, 3]);
    assert!(keys(&cursor, &queue, 0).is_empty());
    assert!(keys(&cursor, &BTreeMap::new(), 10).is_empty());
}

#[test]
fn should_continue_after_the_last_key_and_wrap_without_duplicates() {
    let queue = queue([1, 2, 3, 4, 5]);
    let mut cursor = QueueCursor::new();
    cursor.advance(3);
    assert_eq!(keys(&cursor, &queue, 3), [4, 5, 1]);
    assert_eq!(keys(&cursor, &queue, 10), [4, 5, 1, 2, 3]);
    cursor.advance(5);
    assert_eq!(keys(&cursor, &queue, 2), [1, 2]);
}

#[test]
fn should_continue_after_a_removed_key_and_visit_new_entries() {
    let mut queue = queue([1, 2, 4, 5]);
    let mut cursor = QueueCursor::new();
    cursor.advance(3);
    assert_eq!(keys(&cursor, &queue, 2), [4, 5]);
    queue.insert(3, ());
    queue.insert(6, ());
    assert_eq!(keys(&cursor, &queue, 10), [4, 5, 6, 1, 2, 3]);
}

#[test]
fn should_restart_from_the_first_entry_after_clear() {
    let queue = queue([1, 2, 3]);
    let mut cursor = QueueCursor::new();
    cursor.advance(2);
    cursor.clear();
    assert_eq!(keys(&cursor, &queue, 2), [1, 2]);
}

#[test]
fn should_visit_every_entry_within_one_rotation_for_sparse_queues() {
    for mask in 0u16..256 {
        let queue = queue(
            (0..8)
                .filter(|key| mask & (1 << key) != 0)
                .map(|key| key * 3),
        );
        for limit in 1..=10 {
            for start in 0..=25 {
                let mut cursor = QueueCursor::new();
                cursor.advance(start);
                let mut seen = BTreeSet::new();
                for _ in 0..queue.len().div_ceil(limit) {
                    let round = keys(&cursor, &queue, limit);
                    assert_eq!(round.len(), queue.len().min(limit));
                    assert_eq!(round.iter().collect::<BTreeSet<_>>().len(), round.len());
                    if let Some(last) = round.last() {
                        cursor.advance(*last);
                    }
                    seen.extend(round);
                }
                assert_eq!(seen, queue.keys().copied().collect());
            }
        }
    }
}
