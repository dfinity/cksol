use crate::{
    constants::{GET_BALANCE_CYCLES, RENT_EXEMPTION_THRESHOLD},
    deposit::sweep::{deposit_sol, deposit_status, sweepable_amount},
    guard::process_deposit_guard,
    state::{event::EventType, read_state, reset_state},
    storage::{reset_events, with_event_iter},
    test_fixtures::{
        BLOCK_INDEX, DEPOSIT_CONSOLIDATION_FEE, EventsAssert, MINIMUM_DEPOSIT_AMOUNT,
        PROCESS_DEPOSIT_REQUIRED_CYCLES, account,
        deposit::{DEPOSIT_AMOUNT, DEPOSITOR_ACCOUNT, deposit_id as manual_deposit_id},
        events, init_schnorr_master_key, init_state,
        runtime::TestCanisterRuntime,
    },
};
use assert_matches::assert_matches;
use candid::Principal;
use cksol_types::{DepositSolError, DepositSolStatus, InsufficientCyclesError, Lamport};
use ic_canister_runtime::IcError;
use ic_cdk::call::RejectCode;
use icrc_ledger_types::icrc1::account::Account;
use sol_rpc_types::MultiRpcResult;

const GET_BALANCE_REFUND: u128 = GET_BALANCE_CYCLES / 2;
const EXPLICIT_DEFAULT_SUBACCOUNT: Account = Account {
    subaccount: Some([0; 32]),
    ..DEPOSITOR_ACCOUNT
};

#[test]
fn should_compute_sweepable_amount_above_rent_exemption_threshold() {
    for (balance, expected) in [
        (0, 0),
        (RENT_EXEMPTION_THRESHOLD - 1, 0),
        (RENT_EXEMPTION_THRESHOLD, 0),
        (RENT_EXEMPTION_THRESHOLD + 1, 1),
        (Lamport::MAX, Lamport::MAX - RENT_EXEMPTION_THRESHOLD),
    ] {
        assert_eq!(sweepable_amount(balance), expected, "balance {balance}");
    }
}

#[test]
fn should_report_unknown_deposit_as_not_found() {
    init_state();

    assert_eq!(deposit_status(0), DepositSolStatus::NotFound);
}

#[tokio::test]
async fn should_fail_if_insufficient_cycles_attached() {
    init_state();
    let runtime =
        TestCanisterRuntime::new().add_msg_cycles_available(PROCESS_DEPOSIT_REQUIRED_CYCLES - 1);

    let result = deposit_sol(&runtime, DEPOSITOR_ACCOUNT).await;

    assert_eq!(
        result,
        Err(DepositSolError::InsufficientCycles(
            InsufficientCyclesError {
                expected: PROCESS_DEPOSIT_REQUIRED_CYCLES,
                received: PROCESS_DEPOSIT_REQUIRED_CYCLES - 1,
            }
        ))
    );
    assert!(runtime.msg_cycles_accepted().is_empty());
    EventsAssert::assert_no_events_recorded();
}

#[tokio::test]
async fn should_fail_and_charge_balance_read_if_get_balance_is_rejected() {
    init_state();
    init_schnorr_master_key();
    let runtime = runtime().add_stub_error(IcError::CallRejected {
        code: RejectCode::SysTransient,
        message: "SOL RPC canister is stopped".to_string(),
    });

    let result = deposit_sol(&runtime, DEPOSITOR_ACCOUNT).await;

    assert_matches!(
        result,
        Err(DepositSolError::TemporarilyUnavailable(e)) => assert!(e.contains("Inter-canister call rejected"))
    );
    assert_eq!(
        runtime.msg_cycles_accepted(),
        [GET_BALANCE_CYCLES - GET_BALANCE_REFUND]
    );
    EventsAssert::assert_no_events_recorded();
}

#[tokio::test]
async fn should_fail_and_charge_balance_read_if_balance_below_minimum() {
    init_state();
    init_schnorr_master_key();
    let runtime = runtime().add_get_balance_response(MINIMUM_DEPOSIT_AMOUNT - 1);

    let result = deposit_sol(&runtime, DEPOSITOR_ACCOUNT).await;

    assert_eq!(
        result,
        Err(DepositSolError::ValueTooSmall {
            balance: MINIMUM_DEPOSIT_AMOUNT - 1,
            minimum_deposit_amount: MINIMUM_DEPOSIT_AMOUNT,
        })
    );
    assert_eq!(
        runtime.msg_cycles_accepted(),
        [GET_BALANCE_CYCLES - GET_BALANCE_REFUND]
    );
    EventsAssert::assert_no_events_recorded();
}

#[tokio::test]
async fn should_queue_deposits_from_minimum_with_sequential_ids() {
    init_state();
    init_schnorr_master_key();
    let other_account = account(7);

    for (expected_id, depositor, balance) in [
        (0, DEPOSITOR_ACCOUNT, MINIMUM_DEPOSIT_AMOUNT),
        (1, other_account, MINIMUM_DEPOSIT_AMOUNT + 1),
    ] {
        let runtime = runtime().add_get_balance_response(balance);

        let deposit_id = deposit_sol(&runtime, depositor).await;

        assert_eq!(deposit_id, Ok(expected_id));
        assert_eq!(
            deposit_status(expected_id),
            DepositSolStatus::Queued {
                sweepable_amount: balance - RENT_EXEMPTION_THRESHOLD
            }
        );
        assert_eq!(
            runtime.msg_cycles_accepted(),
            [GET_BALANCE_CYCLES - GET_BALANCE_REFUND + DEPOSIT_CONSOLIDATION_FEE]
        );
    }
    assert_eq!(deposit_status(2), DepositSolStatus::NotFound);
    EventsAssert::from_recorded()
        .expect_event_eq(queued_deposit_event(
            0,
            DEPOSITOR_ACCOUNT,
            MINIMUM_DEPOSIT_AMOUNT - RENT_EXEMPTION_THRESHOLD,
        ))
        .expect_event_eq(queued_deposit_event(
            1,
            other_account,
            MINIMUM_DEPOSIT_AMOUNT + 1 - RENT_EXEMPTION_THRESHOLD,
        ))
        .assert_no_more_events();
}

#[tokio::test]
async fn should_fail_while_process_deposit_is_in_progress() {
    enum ProcessDepositState {
        CallRunning,
        DepositAccepted,
        DepositAwaitingConsolidation,
    }
    for process_deposit_state in [
        ProcessDepositState::CallRunning,
        ProcessDepositState::DepositAccepted,
        ProcessDepositState::DepositAwaitingConsolidation,
    ] {
        reset_state();
        reset_events();
        init_state();
        let _process_deposit_guard = match process_deposit_state {
            ProcessDepositState::CallRunning => {
                Some(process_deposit_guard(DEPOSITOR_ACCOUNT).unwrap())
            }
            ProcessDepositState::DepositAccepted => {
                events::accept_deposit(manual_deposit_id(), DEPOSIT_AMOUNT);
                None
            }
            ProcessDepositState::DepositAwaitingConsolidation => {
                events::accept_deposit(manual_deposit_id(), DEPOSIT_AMOUNT);
                events::mint_deposit(manual_deposit_id(), BLOCK_INDEX);
                None
            }
        };
        let events_before = EventsAssert::from_recorded();
        let runtime =
            TestCanisterRuntime::new().add_msg_cycles_available(PROCESS_DEPOSIT_REQUIRED_CYCLES);

        let result = deposit_sol(&runtime, EXPLICIT_DEFAULT_SUBACCOUNT).await;

        assert_matches!(
            result,
            Err(DepositSolError::TemporarilyUnavailable(e)) => assert!(e.contains("awaiting consolidation"))
        );
        assert!(runtime.msg_cycles_accepted().is_empty());
        assert_eq!(EventsAssert::from_recorded(), events_before);
    }
}

#[tokio::test]
async fn should_return_existing_deposit_id_without_reading_balance() {
    assert_second_call_returns_same_deposit(DEPOSITOR_ACCOUNT, DEPOSITOR_ACCOUNT).await;
}

#[tokio::test]
async fn should_return_same_deposit_for_explicit_default_subaccount() {
    assert_second_call_returns_same_deposit(DEPOSITOR_ACCOUNT, EXPLICIT_DEFAULT_SUBACCOUNT).await;
}

#[tokio::test]
async fn should_return_same_deposit_for_omitted_default_subaccount() {
    assert_second_call_returns_same_deposit(EXPLICIT_DEFAULT_SUBACCOUNT, DEPOSITOR_ACCOUNT).await;
}

#[tokio::test]
#[should_panic(expected = "the owner must be non-anonymous")]
async fn should_reject_anonymous_owner() {
    let anonymous_account = Account {
        owner: Principal::anonymous(),
        subaccount: None,
    };

    let _ = deposit_sol(&TestCanisterRuntime::new(), anonymous_account).await;
}

async fn assert_second_call_returns_same_deposit(first: Account, second: Account) {
    init_state();
    init_schnorr_master_key();
    let deposit_id = deposit_sol(
        &runtime().add_get_balance_response(MINIMUM_DEPOSIT_AMOUNT),
        first,
    )
    .await
    .expect("first deposit should be queued");
    EventsAssert::from_recorded()
        .expect_event_eq(queued_deposit_event(
            deposit_id,
            first,
            MINIMUM_DEPOSIT_AMOUNT - RENT_EXEMPTION_THRESHOLD,
        ))
        .assert_no_more_events();
    let num_events_after_first_call = with_event_iter(|events| events.count());

    let runtime =
        TestCanisterRuntime::new().add_msg_cycles_available(PROCESS_DEPOSIT_REQUIRED_CYCLES);
    let result = deposit_sol(&runtime, second).await;

    assert_eq!(result, Ok(deposit_id));
    assert!(runtime.msg_cycles_accepted().is_empty());
    assert_eq!(
        read_state(|state| state.in_flight_deposit_id(&second)),
        Some(deposit_id)
    );
    assert_eq!(
        with_event_iter(|events| events.count()),
        num_events_after_first_call
    );
}

fn queued_deposit_event(deposit_id: u64, account: Account, sweepable_amount: Lamport) -> EventType {
    EventType::QueuedDeposit {
        deposit_id,
        account,
        sweepable_amount,
    }
}

/// Runtime for a `deposit_sol` call that makes a `getBalance` call
/// whose stub response or error the caller chains.
fn runtime() -> TestCanisterRuntime {
    TestCanisterRuntime::new()
        .with_increasing_time()
        .expecting_charges()
        .add_msg_cycles_available(PROCESS_DEPOSIT_REQUIRED_CYCLES)
        .add_msg_cycles_refunded(GET_BALANCE_REFUND)
}

trait GetBalanceRuntimeExt: Sized {
    fn add_get_balance_response(self, balance: Lamport) -> Self;
}

impl GetBalanceRuntimeExt for TestCanisterRuntime {
    fn add_get_balance_response(self, balance: Lamport) -> Self {
        self.add_stub_response(MultiRpcResult::<Lamport>::Consistent(Ok(balance)))
    }
}
