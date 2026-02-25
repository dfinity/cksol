//! Candid-compatible event types for the ckSOL minter.

use crate::{InitArgs, UpgradeArgs};
use candid::CandidType;
use serde::Deserialize;

/// A minter event that can be serialized to Candid.
#[derive(Clone, Debug, CandidType, Deserialize)]
pub struct Event {
    /// The canister time at which the minter generated this event.
    pub timestamp: u64,
    /// The event type.
    pub payload: EventType,
}

/// The type of a minter event.
#[derive(Clone, Debug, CandidType, Deserialize)]
pub enum EventType {
    /// The minter initialization event.
    /// Must be the first event in the log.
    Init(InitArgs),
    /// The minter upgraded with the specified arguments.
    Upgrade(UpgradeArgs),
}

/// Arguments for the `get_events` endpoint.
#[derive(Clone, Debug, CandidType, Deserialize)]
pub struct GetEventsArgs {
    /// The index of the first event to return.
    pub start: u64,
    /// The maximum number of events to return.
    pub length: u64,
}

/// The result of a `get_events` call.
#[derive(Clone, Debug, CandidType, Deserialize)]
pub struct GetEventsResult {
    /// The events in the requested range.
    pub events: Vec<Event>,
    /// The total number of events in the log.
    pub total_event_count: u64,
}
