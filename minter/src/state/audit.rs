use crate::state::State;
use crate::state::event::{Event, EventType};
use crate::storage;

/// Records the given event payload in the event log and updates the state to reflect the change.
pub fn process_event(state: &mut State, payload: EventType) {
    apply_state_transition(state, &payload);
    storage::record_event(payload);
}

/// Updates the state to reflect the given state transition.
fn apply_state_transition(state: &mut State, payload: &EventType) {
    todo!()
}

pub fn replay_events<T: IntoIterator<Item = Event>>(events: T) -> State {
    todo!()
}
