use crate::{
    runtime::CanisterRuntime,
    state::{
        State,
        audit::{process_event, replay_events},
        event::EventType,
        init_once_state, mutate_state,
    },
    storage::{
        migrate_event_log, record_event, total_event_count, with_event_iter,
        with_unstable_metrics_mut,
    },
};
use canlog::log;
use cksol_types_internal::{InitArgs, UpgradeArgs, log::Priority};

/// One-time migration: converts legacy `AcceptedManualDeposit` events (CBOR index 2,
/// no `source` field) to `AcceptedDeposit { source: Manual }` (same CBOR index).
///
/// Safe to call multiple times. Remove after the migration has been confirmed.
fn migrate_accepted_manual_deposit_events() {
    migrate_event_log();
}

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

    migrate_accepted_manual_deposit_events();
    init_once_state(with_event_iter(|events| replay_events(events)));
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
    // TODO: replace this with a macro, similar as in sol-rpc-canister.
    with_unstable_metrics_mut(|m| m.post_upgrade_instructions_consumed = instructions_consumed);
}
