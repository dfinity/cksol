use crate::{
    deposit::automatic::watch_deposit_address,
    guard::deposit_sol_guard,
    state::{MAX_WATCHED_ADDRESSES, State, mutate_state, read_state},
    test_fixtures::{
        AUTOMATIC_DEPOSIT_FEE, EventsAssert, HOUR_NANOS, MINIMUM_DEPOSIT_AMOUNT, MINTER_ACCOUNT,
        MINUTE_NANOS, REGISTERED_AT, WATCH_WINDOW_NANOS, account, at, deposit_address,
        events::{
            fail_transaction, quarantine_sweep, queue_deposit, submit_sweep, succeed_transaction,
        },
        flow::deposit::DepositFlow,
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

    let result = watch_deposit_address(&runtime(), account(1));

    assert_eq!(
        result,
        Ok(WatchDepositAddressOk {
            address: deposit_address(account(1)).into(),
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
    assert_eq!(read_state(|s| s.watchlist().len()), 1);
}

#[test]
fn should_report_the_stored_registration_without_extending_it() {
    setup();
    let first = watch_deposit_address(&runtime(), account(1)).unwrap();
    mutate_state(|s| {
        s.watchlist_mut()
            .record_scan(at(REGISTERED_AT + 1), &account(1))
    });

    let second =
        watch_deposit_address(&runtime_at(REGISTERED_AT + HOUR_NANOS), account(1)).unwrap();

    assert_eq!(
        second.status,
        WatchDepositAddressStatus::Watching {
            valid_until: REGISTERED_AT + WATCH_WINDOW_NANOS,
            scan_count: 1,
            last_scanned_at: Some(REGISTERED_AT + 1),
            next_scan_at: Some(REGISTERED_AT + MINUTE_NANOS),
        }
    );
    assert_eq!(second.address, first.address);
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
    let minted = DepositFlow::queue(account(1), MINIMUM_DEPOSIT_AMOUNT)
        .sweep(signature(0xAA))
        .succeed()
        .credit()
        .single_pending_mint()
        .mint(7);

    let result = watch_deposit_address(&runtime(), account(1)).unwrap();

    assert_matches!(
        result.status,
        WatchDepositAddressStatus::Watching { scan_count: 0, .. }
    );
    assert_eq!(result.latest_deposit_id, Some(minted.deposit_id));
}

#[test]
fn should_register_anew_once_the_deposit_is_dropped() {
    setup();
    queue_deposit(0, account(1), MINIMUM_DEPOSIT_AMOUNT);
    submit_sweep(signature(0xAA), vec![0]);
    fail_transaction(signature(0xAA));

    let result = watch_deposit_address(&runtime(), account(1)).unwrap();

    assert_matches!(
        result.status,
        WatchDepositAddressStatus::Watching { scan_count: 0, .. }
    );
    assert_eq!(result.latest_deposit_id, Some(0));
}

#[test]
fn should_reject_an_account_whose_latest_deposit_is_quarantined() {
    setup();
    queue_deposit(0, account(1), MINIMUM_DEPOSIT_AMOUNT);
    submit_sweep(signature(0xAA), vec![0]);
    succeed_transaction(signature(0xAA));
    quarantine_sweep(signature(0xAA));
    let state_before = current_state();

    let result = watch_deposit_address(&runtime(), account(1));

    assert_eq!(
        result,
        Err(WatchDepositAddressError::Quarantined { deposit_id: 0 })
    );
    assert_eq!(current_state(), state_before);
}

#[test]
fn should_reject_a_registration_when_the_watchlist_is_full() {
    setup();
    for index in 0..MAX_WATCHED_ADDRESSES.get() {
        watch_deposit_address(&runtime(), account(index)).unwrap();
    }
    let state_before = current_state();

    let result = watch_deposit_address(&runtime(), account(MAX_WATCHED_ADDRESSES.get()));

    assert_eq!(
        result,
        Err(WatchDepositAddressError::TooManyWatchedAddresses)
    );
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

fn queued(account: Account) -> DepositSolId {
    DepositFlow::queue(account, MINIMUM_DEPOSIT_AMOUNT).deposit_id
}

fn swept(account: Account) -> DepositSolId {
    let deposit = DepositFlow::queue(account, MINIMUM_DEPOSIT_AMOUNT);
    let deposit_id = deposit.deposit_id;
    deposit.sweep(signature(0xAA));
    deposit_id
}

fn credited(account: Account) -> DepositSolId {
    let deposit = DepositFlow::queue(account, MINIMUM_DEPOSIT_AMOUNT);
    let deposit_id = deposit.deposit_id;
    deposit.sweep(signature(0xAA)).succeed().credit();
    deposit_id
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
