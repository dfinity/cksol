use ic_stable_structures::Storable;
use proptest::prelude::*;
use std::borrow::Cow;

use super::event::Event;
use crate::test_fixtures::arb_event;

proptest! {
    #[test]
    fn event_minicbor_roundtrip(event in arb_event()) {
        let bytes = event.to_bytes();
        let decoded = Event::from_bytes(Cow::Borrowed(&bytes));
        assert_eq!(event, decoded);
    }
}
