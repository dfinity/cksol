use crate::{
    runtime::CanisterRuntime,
    state::{
        State,
        audit::{process_event, replay_events},
        event::EventType,
        init_once_state, mutate_state,
    },
    storage::{purge_unknown_events, record_event, total_event_count, with_event_iter},
};
use canlog::log;
use cksol_types_internal::{InitArgs, UpgradeArgs, log::Priority};

pub fn init<R: CanisterRuntime>(init_args: InitArgs, runtime: R) {
    log!(
        Priority::Info,
        "[init]: initialized minter with arg: {init_args:?}"
    );
    init_once_state(State::try_from(init_args.clone()).expect("ERROR: invalid init args"));
    record_event(EventType::Init(init_args), &runtime);
}

pub fn post_upgrade<R: CanisterRuntime>(upgrade_args: Option<UpgradeArgs>, runtime: R) {
    let start = runtime.instruction_counter();

    let (state, skipped) = with_event_iter(|events| replay_events(events));
    init_once_state(state);

    if skipped > 0 {
        log!(
            Priority::Info,
            "[upgrade]: skipped {skipped} unknown events during replay, purging from stable memory"
        );
        let purged = purge_unknown_events();
        log!(
            Priority::Info,
            "[upgrade]: purged {purged} unknown events from stable memory"
        );
    }

    if let Some(args) = upgrade_args {
        log!(
            Priority::Info,
            "[upgrade]: upgrading minter with arg: {args:?}"
        );
        mutate_state(|s| process_event(s, EventType::Upgrade(args), &runtime))
    }

    let end = runtime.instruction_counter();

    let event_count = total_event_count();
    let instructions_consumed = end - start;

    log!(
        Priority::Info,
        "[upgrade]: replaying {event_count} events consumed {instructions_consumed} instructions ({} instructions per event on average)",
        instructions_consumed / event_count
    );
}
