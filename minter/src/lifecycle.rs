use crate::{
    runtime::CanisterRuntime,
    state::{
        State,
        audit::{process_event, replay_events},
        event::EventType,
        init_once_state, mutate_state,
    },
    storage::{record_event, total_event_count, with_event_iter, with_unstable_metrics_mut},
};
use canlog::log;
use cksol_types_internal::{InitArgs, UpgradeArgs, log::Priority};

#[cfg(test)]
mod tests;

pub fn init<R: CanisterRuntime>(init_args: InitArgs, runtime: R) {
    log!(
        Priority::Info,
        "[init]: initialized minter with arg: {init_args:?}"
    );
    init_once_state(State::try_from(init_args.clone()).expect("ERROR: invalid init args"));
    record_event(EventType::Init(init_args), &runtime);
}

/// Records the watchlist of the automatic deposit flow, which lives on the heap, as a
/// snapshot event, so that replaying the event log after the upgrade restores it. The
/// snapshot is recorded even when the watchlist is empty, since replay restores the latest
/// one: skipping it would let an older snapshot resurrect registrations evicted since.
pub fn pre_upgrade<R: CanisterRuntime>(runtime: R) {
    mutate_state(|s| {
        let snapshot = s.watchlist().snapshot();
        log!(
            Priority::Info,
            "[pre_upgrade]: recording the snapshot of {} watched deposit addresses",
            snapshot.entries.len()
        );
        process_event(s, EventType::WatchedDepositAddresses(snapshot), &runtime);
    });
}

pub fn post_upgrade<R: CanisterRuntime>(upgrade_args: Option<UpgradeArgs>, runtime: R) {
    let start = runtime.instruction_counter();

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
