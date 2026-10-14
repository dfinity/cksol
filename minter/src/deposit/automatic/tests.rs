use crate::{
    deposit::automatic::watch_deposit_address,
    guard::deposit_sol_guard,
    state::{MAX_WATCHED_ADDRESSES, mutate_state, read_state},
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
use cksol_types::{WatchDepositAddressError, WatchDepositAddressOk, WatchDepositAddressStatus};
use cksol_types_internal::InitArgs;
use icrc_ledger_types::icrc1::account::Account;

#[test]
fn should_fail_while_the_automatic_deposit_fee_is_not_configured() {
    init_state_with_args(InitArgs {
        automatic_deposit_fee: None,
        ..valid_init_args()
    });
    init_schnorr_master_key();

    let result = watch_deposit_address(&runtime(), account(1));

    assert_matches!(
        result,
        Err(WatchDepositAddressError::TemporarilyUnavailable(e)) => assert!(e.contains("automatic deposit fee"))
    );
    assert!(read_state(|s| s.watchlist().is_empty()));
}

#[test]
fn should_fail_while_the_minter_public_key_is_unavailable() {
    init_state();

    let result = watch_deposit_address(&runtime(), account(1));

    assert_matches!(
        result,
        Err(WatchDepositAddressError::TemporarilyUnavailable(e)) => assert!(e.contains("minter public key"))
    );
    assert!(read_state(|s| s.watchlist().is_empty()));
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
            next_scan_at: Some(REGISTERED_AT + 1 + MINUTE_NANOS),
        }
    );
    assert_eq!(second.address, first.address);
    assert_eq!(read_state(|s| s.watchlist().len()), 1);
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
fn should_report_a_queued_deposit_as_detected_without_registering() {
    setup();
    let deposit = DepositFlow::queue(account(1), MINIMUM_DEPOSIT_AMOUNT);

    let result = watch_deposit_address(&runtime(), account(1)).unwrap();

    assert_eq!(
        result.status,
        WatchDepositAddressStatus::Detected {
            deposit_id: deposit.deposit_id
        }
    );
    assert_eq!(result.latest_deposit_id, Some(deposit.deposit_id));
    assert!(read_state(|s| s.watchlist().is_empty()));
}

#[test]
fn should_report_a_swept_deposit_as_detected_without_registering() {
    setup();
    let deposit = DepositFlow::queue(account(1), MINIMUM_DEPOSIT_AMOUNT);
    deposit.sweep(signature(0xAA));

    let result = watch_deposit_address(&runtime(), account(1)).unwrap();

    assert_eq!(
        result.status,
        WatchDepositAddressStatus::Detected {
            deposit_id: deposit.deposit_id
        }
    );
    assert!(read_state(|s| s.watchlist().is_empty()));
}

#[test]
fn should_report_a_credited_deposit_as_detected_without_registering() {
    setup();
    let deposit = DepositFlow::queue(account(1), MINIMUM_DEPOSIT_AMOUNT);
    deposit.sweep(signature(0xAA)).succeed().credit();

    let result = watch_deposit_address(&runtime(), account(1)).unwrap();

    assert_eq!(
        result.status,
        WatchDepositAddressStatus::Detected {
            deposit_id: deposit.deposit_id
        }
    );
    assert!(read_state(|s| s.watchlist().is_empty()));
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

    let result = watch_deposit_address(&runtime(), account(1));

    assert_eq!(
        result,
        Err(WatchDepositAddressError::Quarantined { deposit_id: 0 })
    );
    assert!(read_state(|s| s.watchlist().is_empty()));
}

#[test]
fn should_reject_a_registration_when_the_watchlist_is_full() {
    setup();
    for index in 0..MAX_WATCHED_ADDRESSES.get() {
        watch_deposit_address(&runtime(), account(index)).unwrap();
    }

    let result = watch_deposit_address(&runtime(), account(MAX_WATCHED_ADDRESSES.get()));

    assert_eq!(
        result,
        Err(WatchDepositAddressError::TooManyWatchedAddresses)
    );
    assert_matches!(
        watch_deposit_address(&runtime(), account(0))
            .unwrap()
            .status,
        WatchDepositAddressStatus::Watching { .. }
    );
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

fn runtime() -> TestCanisterRuntime {
    runtime_at(REGISTERED_AT)
}

fn runtime_at(now: u64) -> TestCanisterRuntime {
    TestCanisterRuntime::new().with_increasing_time_from(now)
}
