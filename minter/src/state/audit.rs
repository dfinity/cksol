use crate::{
    runtime::CanisterRuntime,
    state::{
        QueuedDeposit, SchnorrPublicKey, State,
        event::{Event, EventType},
        watchlist::RearmError,
    },
    storage,
    utils::timed_sized_map::Timestamp,
};
use canlog::log;
use cksol_types_internal::log::Priority;

/// Records the given event payload in the event log and updates the state to reflect the change.
pub fn process_event<R: CanisterRuntime>(state: &mut State, payload: EventType, runtime: &R) {
    apply_state_transition(state, &payload, runtime.time());
    storage::record_event(payload, runtime);
}

/// Records the given `FailedTransaction` or `ExpiredTransaction` event of a sweep, which
/// drops its deposits, then puts the accounts of its automatic deposits back on the
/// watchlist for a fresh window, so that the funds still on their deposit addresses are
/// swept again without the user's intervention. Every sweep drop goes through here.
///
/// The re-arm is not part of the state transition of the event: the watchlist is transient,
/// so replaying it from the event log would not see the registrations it was made against.
pub fn process_dropped_sweep<R: CanisterRuntime>(
    state: &mut State,
    payload: EventType,
    runtime: &R,
) {
    let signature = match &payload {
        EventType::FailedTransaction { signature }
        | EventType::ExpiredTransaction { signature } => *signature,
        _ => panic!("BUG: {payload:?} does not drop a transaction"),
    };
    let deposits: Vec<_> = state
        .deposits()
        .swept()
        .get(&signature)
        .unwrap_or_else(|| panic!("BUG: cannot drop sweep {signature} that is not swept"))
        .deposits()
        .iter()
        .map(|(deposit_id, deposit)| (*deposit_id, *deposit))
        .collect();
    process_event(state, payload, runtime);
    let now = Timestamp::from_nanos(runtime.time());
    for (deposit_id, deposit) in deposits {
        match state.watchlist_mut().rearm(now, &deposit) {
            Ok(_) => log!(
                Priority::Info,
                "Watching account {} again after its deposit {deposit_id} was dropped with sweep {signature}",
                deposit.account
            ),
            Err(RearmError::ManualDeposit) => {}
            Err(RearmError::WatchlistFull) => log!(
                Priority::Info,
                "Failed to watch account {} again after its deposit {deposit_id} was dropped with sweep {signature}: the watchlist is full",
                deposit.account
            ),
        }
    }
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
        EventType::SubmittedTransaction {
            signature,
            message,
            signers,
            purpose,
        } => {
            state.process_transaction_submitted(signature, message, signers, purpose, timestamp);
        }
        EventType::SucceededTransaction { signature } => {
            state.process_transaction_succeeded(signature);
        }
        EventType::FailedTransaction { signature } => {
            state.process_transaction_failed(signature);
        }
        EventType::ExpiredTransaction { signature } => {
            state.process_transaction_expired(signature);
        }
        EventType::QueuedDeposit {
            deposit_id,
            account,
            address,
            balance,
            deposit_fee,
        } => {
            state.process_queued_deposit(
                *deposit_id,
                QueuedDeposit {
                    account: *account,
                    address: *address,
                    balance: *balance,
                    deposit_fee: *deposit_fee,
                },
                timestamp,
            );
        }
        EventType::CreditedSweep {
            signature,
            amount_received,
            mints,
        } => {
            state.process_credited_sweep(signature, *amount_received, mints, timestamp);
        }
        EventType::QuarantinedSweep { signature } => {
            state.process_quarantined_sweep(signature);
        }
        EventType::MinterPublicKeyFetched {
            public_key,
            chain_code,
        } => {
            state.cache_minter_public_key(SchnorrPublicKey {
                public_key: *public_key,
                chain_code: *chain_code,
            });
        }
        EventType::MintedSweptDeposit {
            deposit_id,
            mint_block_index,
        } => {
            state.process_minted_swept_deposit(*deposit_id, mint_block_index);
        }
        EventType::QuarantinedPendingMint { deposit_id } => {
            state.process_quarantined_pending_mint(*deposit_id);
        }
        EventType::CreatedWithdrawalTransaction {
            burn_indices,
            nonce_account,
            nonce_value,
        } => {
            state.process_transaction_created(burn_indices, nonce_account, *nonce_value);
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
