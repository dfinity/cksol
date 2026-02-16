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
    match payload {
        EventType::Init(init_arg) => {
            panic!("BUG: state re-initialization is not allowed: {init_arg:?}");
        }
        EventType::Upgrade(upgrade_arg) => {
            state
                .upgrade(upgrade_arg.clone())
                .expect("applying upgrade event should succeed");
        }
    }
}

pub fn replay_events<T: IntoIterator<Item = Event>>(events: T) -> State {
    let mut events_iter = events.into_iter();
    let mut state = match events_iter
        .next()
        .expect("the event log should not be empty")
    {
        Event {
            payload: EventType::Init(init_arg),
            ..
        } => State::try_from(init_arg).expect("BUG: state initialization should succeed"),
        other => panic!("ERROR: the first event must be an Init event, got: {other:?}"),
    };
    for event in events_iter {
        apply_state_transition(&mut state, &event.payload);
    }
    state
}
