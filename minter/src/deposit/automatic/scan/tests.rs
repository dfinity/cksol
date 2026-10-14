use crate::{
    constants::{
        MAX_ADDRESSES_PER_GET_MULTIPLE_ACCOUNTS, MAX_CONCURRENT_RPC_CALLS, RENT_EXEMPTION_THRESHOLD,
    },
    deposit::automatic::scan::{Scan, apply_balance_read, scan_watched_addresses},
    guard::deposit_sol_guard,
    state::{
        AutomaticDeposit, TaskType, WatchedAddress, event::EventType, mutate_state, read_state,
    },
    storage::{ScanOutcome, deposit_address_scan_count},
    test_fixtures::{
        AUTOMATIC_DEPOSIT_FEE, EventsAssert, MINIMUM_DEPOSIT_AMOUNT, MINUTE_NANOS, REGISTERED_AT,
        WATCH_WINDOW_NANOS, account, at, deposit_address, flow::deposit::DepositFlow,
        init_schnorr_master_key, init_state, init_state_with_args, queued_deposit_of,
        runtime::TestCanisterRuntime, valid_init_args, watch,
    },
};
use cksol_types::DepositSolStatus;
use cksol_types_internal::InitArgs;
use ic_canister_runtime::IcError;
use sol_rpc_types::{RpcConfig, RpcSources};

const VALID_UNTIL: u64 = REGISTERED_AT + WATCH_WINDOW_NANOS;
const TICK_AT: u64 = REGISTERED_AT + MINUTE_NANOS;

#[tokio::test]
async fn should_return_early_without_due_addresses() {
    setup();
    let runtime = runtime_at(TICK_AT);

    scan_watched_addresses(runtime.clone()).await;

    assert!(runtime.sent_update_calls().is_empty());
    EventsAssert::assert_no_events_recorded();
}

#[tokio::test]
async fn should_return_early_if_task_already_active() {
    setup();
    watch(at(REGISTERED_AT), account(1));
    mutate_state(|s| {
        s.active_tasks_mut().insert(TaskType::ScanWatchedAddresses);
    });
    let runtime = runtime_at(TICK_AT);

    scan_watched_addresses(runtime.clone()).await;

    assert!(runtime.sent_update_calls().is_empty());
}

#[tokio::test]
async fn should_queue_a_deposit_reaching_the_minimum_with_the_fee_in_force() {
    setup();
    watch(at(REGISTERED_AT), account(1));
    let runtime =
        runtime_at(TICK_AT).add_get_multiple_accounts_response(&[Some(MINIMUM_DEPOSIT_AMOUNT)]);

    scan_watched_addresses(runtime.clone()).await;

    let expected = queued_deposit_of(
        account(1),
        MINIMUM_DEPOSIT_AMOUNT - RENT_EXEMPTION_THRESHOLD,
    );
    EventsAssert::from_recorded()
        .expect_event_eq(EventType::QueuedDeposit {
            deposit_id: 0,
            account: account(1),
            address: expected.address,
            balance: expected.balance,
            automatic: Some(AutomaticDeposit {
                fee: AUTOMATIC_DEPOSIT_FEE,
                valid_until: VALID_UNTIL,
            }),
        })
        .assert_no_more_events();
    read_state(|s| {
        assert!(s.watchlist().is_empty());
        assert_eq!(s.deposits().in_flight_id(&account(1)), Some(0));
        assert_eq!(
            s.deposits().status(0),
            DepositSolStatus::Queued {
                sweepable_amount: MINIMUM_DEPOSIT_AMOUNT - RENT_EXEMPTION_THRESHOLD
            }
        );
    });
    assert_eq!(deposit_address_scan_count(ScanOutcome::Queued), 1);
}

#[tokio::test]
async fn should_advance_the_schedule_below_the_minimum_and_treat_a_missing_account_as_empty() {
    setup();
    watch(at(REGISTERED_AT), account(1));
    watch(at(REGISTERED_AT), account(2));
    let runtime = runtime_at(TICK_AT)
        .add_get_multiple_accounts_response(&[Some(MINIMUM_DEPOSIT_AMOUNT - 1), None]);

    scan_watched_addresses(runtime.clone()).await;

    EventsAssert::assert_no_events_recorded();
    let scanned_at = TICK_AT + 1;
    for account in [account(1), account(2)] {
        assert_eq!(
            read_state(|s| s.watchlist().get(at(scanned_at), &account)),
            Some(WatchedAddress {
                account,
                address: deposit_address(account),
                valid_until: at(VALID_UNTIL),
                scan_count: 1,
                last_scanned_at: Some(at(scanned_at)),
                next_scan_at: Some(at(scanned_at + MINUTE_NANOS)),
            })
        );
    }
    assert_eq!(deposit_address_scan_count(ScanOutcome::BelowMinimum), 2);
}

#[tokio::test]
async fn should_read_the_addresses_of_the_due_registrations_only() {
    setup();
    watch(at(REGISTERED_AT), account(1));
    watch(at(REGISTERED_AT), account(2));
    mutate_state(|s| s.watchlist_mut().record_scan(at(TICK_AT - 1), &account(2)));
    let runtime = runtime_at(TICK_AT).add_get_multiple_accounts_response(&[Some(0)]);

    scan_watched_addresses(runtime.clone()).await;

    assert_eq!(
        runtime.requested_addresses(),
        vec![vec![deposit_address(account(1)).to_string()]]
    );
}

#[tokio::test]
async fn should_skip_the_accounts_with_a_deposit_sol_call_in_progress() {
    setup();
    watch(at(REGISTERED_AT), account(1));
    watch(at(REGISTERED_AT), account(2));
    let _guard = deposit_sol_guard(account(1)).unwrap();
    let runtime = runtime_at(TICK_AT).add_get_multiple_accounts_response(&[Some(0)]);

    scan_watched_addresses(runtime.clone()).await;

    assert_eq!(
        runtime.requested_addresses(),
        vec![vec![deposit_address(account(2)).to_string()]]
    );
}

#[tokio::test]
async fn should_skip_the_tick_while_the_automatic_deposit_fee_is_not_configured() {
    init_state_with_args(InitArgs {
        automatic_deposit_fee: None,
        ..valid_init_args()
    });
    init_schnorr_master_key();
    let watched = watch(at(REGISTERED_AT), account(1));
    let runtime =
        runtime_at(TICK_AT).add_get_multiple_accounts_response(&[Some(MINIMUM_DEPOSIT_AMOUNT)]);

    scan_watched_addresses(runtime.clone()).await;

    assert!(runtime.sent_update_calls().is_empty());
    EventsAssert::assert_no_events_recorded();
    assert_eq!(
        read_state(|s| s.watchlist().get(at(TICK_AT), &account(1))),
        Some(watched)
    );
}

#[tokio::test]
async fn should_evict_expired_registrations_at_each_tick() {
    setup();
    watch(at(REGISTERED_AT), account(1));
    let runtime = runtime_at(VALID_UNTIL + 1);

    scan_watched_addresses(runtime.clone()).await;

    assert!(runtime.sent_update_calls().is_empty());
    assert!(read_state(|s| s.watchlist().is_empty()));
}

#[tokio::test]
async fn should_advance_the_schedule_when_the_call_fails() {
    setup();
    watch(at(REGISTERED_AT), account(1));
    let runtime = runtime_at(TICK_AT).add_stub_error(IcError::CallPerformFailed);

    scan_watched_addresses(runtime.clone()).await;

    EventsAssert::assert_no_events_recorded();
    let scanned_at = TICK_AT + 1;
    assert_eq!(
        read_state(|s| s.watchlist().get(at(scanned_at), &account(1))),
        Some(WatchedAddress {
            account: account(1),
            address: deposit_address(account(1)),
            valid_until: at(VALID_UNTIL),
            scan_count: 1,
            last_scanned_at: Some(at(scanned_at)),
            next_scan_at: Some(at(scanned_at + MINUTE_NANOS)),
        })
    );
    assert_eq!(deposit_address_scan_count(ScanOutcome::Failed), 1);
}

#[tokio::test]
async fn should_read_at_most_a_full_watchlist_in_chunks_of_one_call_each() {
    setup();
    let total = MAX_ADDRESSES_PER_GET_MULTIPLE_ACCOUNTS * MAX_CONCURRENT_RPC_CALLS;
    for index in 0..total {
        watch(at(REGISTERED_AT), account(index));
    }
    let mut runtime = runtime_at(TICK_AT);
    for _ in 0..MAX_CONCURRENT_RPC_CALLS {
        runtime = runtime.add_get_multiple_accounts_response(
            &vec![Some(0); MAX_ADDRESSES_PER_GET_MULTIPLE_ACCOUNTS],
        );
    }

    scan_watched_addresses(runtime.clone()).await;

    let requested = runtime.requested_addresses();
    assert_eq!(requested.len(), MAX_CONCURRENT_RPC_CALLS);
    assert!(
        requested
            .iter()
            .all(|chunk| chunk.len() == MAX_ADDRESSES_PER_GET_MULTIPLE_ACCOUNTS)
    );
    assert_eq!(
        deposit_address_scan_count(ScanOutcome::BelowMinimum),
        total as u64
    );
}

#[test]
fn should_discard_a_read_of_a_registration_replaced_or_expired_meanwhile() {
    setup();
    let stale = watch(at(REGISTERED_AT), account(1));
    mutate_state(|s| s.watchlist_mut().remove(&account(1)));
    let renewed = watch(at(REGISTERED_AT + MINUTE_NANOS), account(1));
    let runtime = runtime_at(TICK_AT);

    let after_replacement =
        apply_balance_read(&runtime, scan_at(TICK_AT), &stale, MINIMUM_DEPOSIT_AMOUNT);
    let after_expiry = apply_balance_read(
        &runtime,
        scan_at(renewed.valid_until.as_nanos() + 1),
        &renewed,
        MINIMUM_DEPOSIT_AMOUNT,
    );

    assert_eq!(after_replacement, None);
    assert_eq!(after_expiry, None);
    EventsAssert::assert_no_events_recorded();
    assert_eq!(
        read_state(|s| s.watchlist().get(at(TICK_AT), &account(1))),
        Some(renewed)
    );
}

#[test]
fn should_discard_a_read_of_an_account_whose_deposit_is_in_flight() {
    setup();
    let watched = watch(at(REGISTERED_AT), account(1));
    let deposit = DepositFlow::queue(account(1), MINIMUM_DEPOSIT_AMOUNT);
    mutate_state(|s| {
        s.watchlist_mut()
            .register(at(REGISTERED_AT), account(1), deposit.address)
            .unwrap()
    });
    let events_before = EventsAssert::from_recorded();

    let outcome = apply_balance_read(
        &runtime_at(TICK_AT),
        scan_at(TICK_AT),
        &watched,
        MINIMUM_DEPOSIT_AMOUNT,
    );

    assert_eq!(outcome, None);
    assert_eq!(EventsAssert::from_recorded(), events_before);
    assert_eq!(
        read_state(|s| s.deposits().in_flight_id(&account(1))),
        Some(deposit.deposit_id)
    );
}

fn setup() {
    init_state();
    init_schnorr_master_key();
}

fn scan_at(scanned_at: u64) -> Scan {
    Scan {
        scanned_at: at(scanned_at),
        automatic_deposit_fee: AUTOMATIC_DEPOSIT_FEE,
    }
}

fn runtime_at(now: u64) -> TestCanisterRuntime {
    TestCanisterRuntime::new().with_increasing_time_from(now)
}

trait RequestedAddressesExt {
    fn requested_addresses(&self) -> Vec<Vec<String>>;
}

impl RequestedAddressesExt for TestCanisterRuntime {
    fn requested_addresses(&self) -> Vec<Vec<String>> {
        self.sent_update_calls()
            .iter()
            .map(|call| {
                assert_eq!(call.method, "jsonRequest");
                let (_sources, _config, request): (RpcSources, Option<RpcConfig>, String) =
                    call.args();
                let request: serde_json::Value = serde_json::from_str(&request).unwrap();
                serde_json::from_value(request["params"][0].clone()).unwrap()
            })
            .collect()
    }
}
