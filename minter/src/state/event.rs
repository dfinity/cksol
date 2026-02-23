use cksol_types_internal::{InitArgs, UpgradeArgs};
use ic_stable_structures::Storable;
use ic_stable_structures::storable::Bound;
use minicbor::{Decode, Encode};
use std::borrow::Cow;

#[derive(Eq, PartialEq, Debug, Decode, Encode)]
pub struct Event {
    /// The canister time at which the minter generated this event.
    #[n(0)]
    pub timestamp: u64,
    /// The event type.
    #[n(1)]
    pub payload: EventType,
}

#[derive(Clone, Eq, PartialEq, Debug, Decode, Encode)]
pub enum EventType {
    /// The minter initialization event.
    /// Must be the first event in the log.
    #[n(0)]
    Init(#[n(0)] InitArgs),
    /// The minter upgraded with the specified arguments.
    #[n(1)]
    Upgrade(#[n(0)] UpgradeArgs),
}

impl Storable for Event {
    fn to_bytes(&self) -> Cow<'_, [u8]> {
        let mut buf = vec![];
        minicbor::encode(self, &mut buf).expect("event encoding should always succeed");
        Cow::Owned(buf)
    }

    fn into_bytes(self) -> Vec<u8> {
        self.to_bytes().into_owned()
    }

    fn from_bytes(bytes: Cow<[u8]>) -> Self {
        minicbor::decode(bytes.as_ref())
            .unwrap_or_else(|e| panic!("failed to decode event bytes {}: {e}", hex::encode(bytes)))
    }

    const BOUND: Bound = Bound::Unbounded;
}
