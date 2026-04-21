use crate::utils::stable_sort_key_map::StableSortKeyMap;
use ic_stable_structures::{Storable, storable::Bound};
use icrc_ledger_types::icrc1::account::Account;
use minicbor::{Decode, Encode};
use std::borrow::Cow;

#[cfg(test)]
mod tests;

/// Per-account state for automated deposit discovery.
///
/// This cache is intentionally separate from the event log: it can be fully
/// reconstructed by redoing the `getSignaturesForAddress` HTTP outcalls, so
/// there is no need to replay events to restore it.
#[derive(Clone, Debug, Default, PartialEq, Encode, Decode)]
pub struct AutomaticDepositCacheEntry {
    /// The number of `getSignaturesForAddress` calls made so far for this account.
    #[n(0)]
    pub get_signatures_calls: u8,
}

impl Storable for AutomaticDepositCacheEntry {
    fn to_bytes(&self) -> Cow<'_, [u8]> {
        let mut buf = vec![];
        minicbor::encode(self, &mut buf)
            .expect("AutomaticDepositCacheEntry encoding should succeed");
        Cow::Owned(buf)
    }

    fn from_bytes(bytes: Cow<[u8]>) -> Self {
        minicbor::decode(bytes.as_ref()).unwrap_or_else(|e| {
            panic!(
                "failed to decode AutomaticDepositCacheEntry: {e} (bytes: {})",
                hex::encode(bytes)
            )
        })
    }

    const BOUND: Bound = Bound::Unbounded;
}

/// Map from `Account` to `AutomaticDepositCacheEntry`, indexed by `next_poll_at`
/// timestamp for ordered iteration. The poll time is stored alongside the cache entry
/// inside the map (not in `AutomaticDepositCacheEntry` itself), analogous to how
/// `InsertionOrderedMap` stores the sequence number alongside the value.
///
/// Accounts that have been stopped from monitoring are stored with index `u64::MAX`
/// so they are retained in the map but never returned by `iter_by_index_up_to`.
pub type AutomaticDepositCache = StableSortKeyMap<Account, u64, AutomaticDepositCacheEntry>;
