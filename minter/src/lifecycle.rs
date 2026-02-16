use cksol_types_internal::{InitArgs, UpgradeArgs};

use crate::state::event::EventType;
use crate::state::{State, init_once_state, mutate_state};
use crate::state::audit::{process_event, replay_events};
use crate::storage::{record_event, total_event_count, with_event_iter};

pub fn init(init_args: InitArgs) {
    // TODO DEFI-2665: logging
    // log!(INFO, "[init]: initialized minter with arg: {:?}", init_arg);
    init_once_state(State::try_from(init_args.clone()).expect("ERROR: invalid init args"));
    record_event(EventType::Init(init_args));
}

pub fn post_upgrade(upgrade_args: Option<UpgradeArgs>) {
    let start = ic_cdk::api::instruction_counter();

    init_once_state(with_event_iter(|events| replay_events(events)));
    if let Some(args) = upgrade_args {
        mutate_state(|s| process_event(s, EventType::Upgrade(args)))
    }

    let end = ic_cdk::api::instruction_counter();

    let event_count = total_event_count();
    let instructions_consumed = end - start;

    // TODO DEFI-2665: logging
    // log!(
    //     INFO,
    //     "[upgrade]: replaying {event_count} events consumed {instructions_consumed} instructions ({} instructions per event on average)",
    //     instructions_consumed / event_count
    // );
}
