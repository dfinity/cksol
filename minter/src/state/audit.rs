use crate::{
    runtime::CanisterRuntime,
    state::{
        State,
        event::{Event, EventType},
    },
    storage,
};

/// Records the given event payload in the event log and updates the state to reflect the change.
pub fn process_event<R: CanisterRuntime>(state: &mut State, payload: EventType, runtime: &R) {
    apply_state_transition(state, &payload, runtime.time());
    storage::record_event(payload, runtime);
}

/// Updates the state to reflect the given state transition.
fn apply_state_transition(state: &mut State, payload: &EventType, timestamp: u64) {
    match payload {
        EventType::Init(init_arg) => {
            panic!("BUG: state re-initialization is not allowed: {init_arg:?}");
        }
        EventType::Upgrade(upgrade_arg) => {
            state
                .upgrade(upgrade_arg.clone())
                .expect("applying upgrade event should succeed");
        }
        EventType::AcceptedWithdrawalRequest(request) => {
            state.process_accepted_withdrawal(request, timestamp);
        }
        EventType::AcceptedDeposit {
            deposit_id,
            deposit_amount,
            amount_to_mint,
        } => {
            state.process_accepted_deposit(deposit_id, deposit_amount, amount_to_mint);
        }
        EventType::QuarantinedDeposit(deposit_id) => state.process_quarantined_deposit(deposit_id),
        EventType::Minted {
            deposit_id,
            mint_block_index,
        } => {
            state.process_mint(deposit_id, mint_block_index);
        }
        EventType::SubmittedTransaction {
            signature,
            message,
            signers,
            slot,
            purpose,
        } => {
            state.process_transaction_submitted(signature, message, signers, *slot, purpose);
        }
        EventType::ResubmittedTransaction {
            old_signature,
            new_signature,
            new_slot,
        } => {
            state.process_transaction_resubmitted(old_signature, new_signature, *new_slot);
        }
        EventType::SucceededTransaction { signature } => {
            state.process_transaction_succeeded(signature);
        }
        EventType::FailedTransaction { signature } => {
            state.process_transaction_failed(signature);
        }
        EventType::ExpiredTransaction { signature } => {
            state.process_transaction_expired(*signature);
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
        apply_state_transition(&mut state, &event.payload, event.timestamp);
    }
    state
}
