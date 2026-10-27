use crate::{
    deposit::automatic::watch_deposit_address,
    guard::deposit_sol_guard,
    state::{MAX_WATCHED_ADDRESSES, State, mutate_state, read_state},
    test_fixtures::{
        AUTOMATIC_DEPOSIT_FEE, EventsAssert, HOUR_NANOS, MINIMUM_DEPOSIT_AMOUNT, MINTER_ACCOUNT,
        MINUTE_NANOS, REGISTERED_AT, WATCH_WINDOW_NANOS, account, at, deposit_address,
        events::{fail_transaction, quarantine_sweep},
        flow::deposit::{CreditedSweepFlow, DepositFlow},
        init_schnorr_master_key, init_state, init_state_with_args,
        runtime::TestCanisterRuntime,
        signature, valid_init_args,
    },
};
use assert_matches::assert_matches;
use candid::Principal;
use cksol_types::{
    DepositSolId, WatchDepositAddressError, WatchDepositAddressOk, WatchDepositAddressStatus,
};
use cksol_types_internal::InitArgs;
use icrc_ledger_types::icrc1::account::Account;

#[test]
fn should_fail_without_registering_while_a_prerequisite_is_unavailable() {
    for (setup_without_prerequisite, unavailable) in [
        (
            setup_without_automatic_deposit_fee as fn(),
            "automatic deposit fee",
        ),
        (setup_without_minter_public_key, "minter public key"),
    ] {
        in_fresh_state(|| {
            setup_without_prerequisite();
            let state_before = current_state();

            let result = watch_deposit_address(&runtime(), account(1));

            assert_matches!(
                result,
                Err(WatchDepositAddressError::TemporarilyUnavailable(e)) => assert!(e.contains(unavailable))
            );
            assert_eq!(current_state(), state_before);
        });
    }
}

#[test]
fn should_register_the_account_without_recording_an_event() {
    setup();
    let with_subaccount = Account {
        subaccount: Some([7; 32]),
        ..account(1)
    };

    let result = watch_deposit_address(&runtime(), with_subaccount);

    assert_eq!(
        result,
        Ok(WatchDepositAddressOk {
            address: deposit_address(with_subaccount).into(),
            minimum_deposit_amount: MINIMUM_DEPOSIT_AMOUNT,
            automatic_deposit_fee: AUTOMATIC_DEPOSIT_FEE,
            status: WatchDepositAddressStatus::Watching {
                valid_until: REGISTERED_AT + WATCH_WINDOW_NANOS,
                scan_count: 0,
                last_scanned_at: None,
                next_scan_at: Some(REGISTERED_AT),
            },
            latest_deposit_id: None,
        })
    );
    EventsAssert::assert_no_events_recorded();
    assert_eq!(
        read_state(|s| s
            .watchlist()
            .iter()
            .map(|watched| watched.account)
            .collect::<Vec<_>>()),
        vec![with_subaccount]
    );
}

#[test]
fn should_report_the_stored_registration_without_extending_it() {
    setup();
    let first = watch_deposit_address(&runtime(), account(1)).unwrap();
    let first_valid_until = assert_matches!(
        first.status,
        WatchDepositAddressStatus::Watching { valid_until, .. } => valid_until
    );
    mutate_state(|s| {
        s.watchlist_mut()
            .record_scan(at(REGISTERED_AT + 1), &account(1))
    });

    let second =
        watch_deposit_address(&runtime_at(REGISTERED_AT + HOUR_NANOS), account(1)).unwrap();

    assert_eq!(
        second,
        WatchDepositAddressOk {
            status: WatchDepositAddressStatus::Watching {
                valid_until: first_valid_until,
                scan_count: 1,
                last_scanned_at: Some(REGISTERED_AT + 1),
                next_scan_at: Some(REGISTERED_AT + MINUTE_NANOS),
            },
            ..first
        }
    );
    assert_eq!(read_state(|s| s.watchlist().len()), 1);
}

#[test]
fn should_report_the_same_registration_before_the_first_scan() {
    setup();
    let first = watch_deposit_address(&runtime(), account(1)).unwrap();

    let second =
        watch_deposit_address(&runtime_at(REGISTERED_AT + HOUR_NANOS), account(1)).unwrap();

    assert_eq!(second, first);
}

#[test]
fn should_register_anew_once_the_window_has_passed() {
    setup();
    watch_deposit_address(&runtime(), account(1)).unwrap();
    let after_window = REGISTERED_AT + WATCH_WINDOW_NANOS + 1;

    let renewed = watch_deposit_address(&runtime_at(after_window), account(1)).unwrap();

    assert_eq!(
        renewed.status,
        WatchDepositAddressStatus::Watching {
            valid_until: after_window + WATCH_WINDOW_NANOS,
            scan_count: 0,
            last_scanned_at: None,
            next_scan_at: Some(after_window),
        }
    );
}

#[test]
fn should_report_an_in_flight_deposit_as_detected_without_registering() {
    for reach_in_flight_stage in [queued as fn(Account) -> DepositSolId, swept, credited] {
        in_fresh_state(|| {
            setup();
            let deposit_id = reach_in_flight_stage(account(1));
            let state_before = current_state();

            let result = watch_deposit_address(&runtime(), account(1)).unwrap();

            assert_eq!(
                result.status,
                WatchDepositAddressStatus::Detected { deposit_id }
            );
            assert_eq!(result.latest_deposit_id, Some(deposit_id));
            assert_eq!(current_state(), state_before);
        });
    }
}

#[test]
fn should_register_anew_once_the_deposit_is_minted() {
    setup();
    let deposit_id = minted(account(1));

    let result = watch_deposit_address(&runtime(), account(1)).unwrap();

    assert_matches!(
        result.status,
        WatchDepositAddressStatus::Watching { scan_count: 0, .. }
    );
    assert_eq!(result.latest_deposit_id, Some(deposit_id));
}

#[test]
fn should_register_anew_once_the_deposit_is_dropped() {
    setup();
    let deposit_id = dropped(account(1));

    let result = watch_deposit_address(&runtime(), account(1)).unwrap();

    assert_matches!(
        result.status,
        WatchDepositAddressStatus::Watching { scan_count: 0, .. }
    );
    assert_eq!(result.latest_deposit_id, Some(deposit_id));
}

#[test]
fn should_reject_an_account_whose_latest_deposit_is_quarantined() {
    for quarantine in [
        quarantined_sweep as fn(Account) -> DepositSolId,
        quarantined_mint,
    ] {
        in_fresh_state(|| {
            setup();
            let deposit_id = quarantine(account(1));
            let state_before = current_state();

            let result = watch_deposit_address(&runtime(), account(1));

            assert_eq!(
                result,
                Err(WatchDepositAddressError::Quarantined { deposit_id })
            );
            assert_eq!(current_state(), state_before);
        });
    }
}

#[test]
fn should_reject_a_registration_when_the_watchlist_is_full() {
    let unwatched = account(MAX_WATCHED_ADDRESSES.get());
    for with_dropped_deposit in [false, true] {
        in_fresh_state(|| {
            setup();
            let latest_deposit_id = with_dropped_deposit.then(|| dropped(unwatched));
            fill_watchlist();
            let state_before = current_state();

            let result = watch_deposit_address(&runtime(), unwatched);

            assert_eq!(
                result,
                Err(WatchDepositAddressError::TooManyWatchedAddresses { latest_deposit_id })
            );
            assert_eq!(current_state(), state_before);
        });
    }
}

#[test]
fn should_report_the_status_without_a_slot_when_the_watchlist_is_full() {
    setup();
    let in_flight = account(MAX_WATCHED_ADDRESSES.get());
    let deposit_id = queued(in_flight);
    let detected = watch_deposit_address(&runtime(), in_flight).unwrap();
    assert_eq!(
        detected.status,
        WatchDepositAddressStatus::Detected { deposit_id }
    );
    let registrations = fill_watchlist();
    let state_before = current_state();

    assert_eq!(
        watch_deposit_address(&runtime(), account(0)),
        Ok(registrations[0].clone())
    );
    assert_eq!(watch_deposit_address(&runtime(), in_flight), Ok(detected));
    assert_eq!(current_state(), state_before);
}

#[test]
fn should_register_while_a_deposit_sol_call_is_in_progress() {
    setup();
    let _guard = deposit_sol_guard(account(1)).unwrap();

    let result = watch_deposit_address(&runtime(), account(1)).unwrap();

    assert_matches!(result.status, WatchDepositAddressStatus::Watching { .. });
}

#[test]
#[should_panic(expected = "the owner must be non-anonymous")]
fn should_reject_an_anonymous_owner() {
    let anonymous_account = Account {
        owner: Principal::anonymous(),
        subaccount: None,
    };

    let _ = watch_deposit_address(&runtime(), anonymous_account);
}

#[test]
#[should_panic(expected = "is not a valid deposit owner")]
fn should_reject_the_minter_as_owner() {
    let _ = watch_deposit_address(&runtime(), MINTER_ACCOUNT);
}

fn setup() {
    init_state();
    init_schnorr_master_key();
}

fn setup_without_automatic_deposit_fee() {
    init_state_with_args(InitArgs {
        automatic_deposit_fee: None,
        ..valid_init_args()
    });
    init_schnorr_master_key();
}

fn setup_without_minter_public_key() {
    init_state();
}

/// Registers the accounts 0 to `MAX_WATCHED_ADDRESSES - 1`, which fills the watchlist, and
/// returns their registrations in that order.
fn fill_watchlist() -> Vec<WatchDepositAddressOk> {
    (0..MAX_WATCHED_ADDRESSES.get())
        .map(|index| watch_deposit_address(&runtime(), account(index)).unwrap())
        .collect()
}

// The helpers below take a deposit of the account to a stage of its life, sweeping it alone
// under `SWEEP`, and return its id.

const SWEEP: usize = 0xAA;

fn queued(account: Account) -> DepositSolId {
    DepositFlow::queue(account, MINIMUM_DEPOSIT_AMOUNT).deposit_id
}

fn swept(account: Account) -> DepositSolId {
    let deposit = DepositFlow::queue(account, MINIMUM_DEPOSIT_AMOUNT);
    deposit.sweep(signature(SWEEP));
    deposit.deposit_id
}

fn dropped(account: Account) -> DepositSolId {
    let deposit_id = swept(account);
    fail_transaction(signature(SWEEP));
    deposit_id
}

fn quarantined_sweep(account: Account) -> DepositSolId {
    let deposit = DepositFlow::queue(account, MINIMUM_DEPOSIT_AMOUNT);
    deposit.sweep(signature(SWEEP)).succeed();
    quarantine_sweep(signature(SWEEP));
    deposit.deposit_id
}

fn credited(account: Account) -> DepositSolId {
    credit(account).0
}

fn minted(account: Account) -> DepositSolId {
    let (deposit_id, sweep) = credit(account);
    sweep.single_pending_mint().mint(7);
    deposit_id
}

fn quarantined_mint(account: Account) -> DepositSolId {
    let (deposit_id, sweep) = credit(account);
    sweep.single_pending_mint().quarantine();
    deposit_id
}

fn credit(account: Account) -> (DepositSolId, CreditedSweepFlow) {
    let deposit = DepositFlow::queue(account, MINIMUM_DEPOSIT_AMOUNT);
    let sweep = deposit.sweep(signature(SWEEP)).succeed().credit();
    (deposit.deposit_id, sweep)
}

fn in_fresh_state(test: impl FnOnce() + Send) {
    std::thread::scope(|scope| {
        scope.spawn(test);
    });
}

fn current_state() -> State {
    read_state(|s| s.clone())
}

fn runtime() -> TestCanisterRuntime {
    runtime_at(REGISTERED_AT)
}

fn runtime_at(now: u64) -> TestCanisterRuntime {
    TestCanisterRuntime::new().with_increasing_time_from(now)
}
