use super::*;
use crate::test_fixtures::arb::arb_cache_entry;
use ic_stable_structures::Storable;
use proptest::{prop_assert_eq, proptest};

proptest! {
    #[test]
    fn storable_roundtrip(entry in arb_cache_entry()) {
        let bytes = entry.to_bytes();
        let restored = AutomaticDepositCacheEntry::from_bytes(bytes);
        prop_assert_eq!(entry, restored);
    }
}
