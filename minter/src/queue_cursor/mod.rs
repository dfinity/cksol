//! Bounded queue selection that continues past entries which need to be retried.

use std::{
    collections::BTreeMap,
    ops::Bound::{Excluded, Unbounded},
};

#[cfg(test)]
mod tests;

/// The last selected key. This cursor is transient and resets after an upgrade.
pub(crate) struct QueueCursor<K>(Option<K>);

impl<K> QueueCursor<K> {
    pub const fn new() -> Self {
        Self(None)
    }

    pub fn clear(&mut self) {
        self.0 = None;
    }
}

impl<K: Copy + Ord> QueueCursor<K> {
    /// Selects at most `limit` entries after the cursor, wrapping once.
    pub fn round<'a, V>(&self, queue: &'a BTreeMap<K, V>, limit: usize) -> Vec<(&'a K, &'a V)> {
        let start = self.0.map_or(Unbounded, Excluded);
        queue
            .range((start, Unbounded))
            .chain(queue.iter())
            .take(queue.len().min(limit))
            .collect()
    }

    pub fn advance(&mut self, key: K) {
        self.0 = Some(key);
    }
}
