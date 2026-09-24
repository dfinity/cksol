use crate::{
    constants::{GET_BALANCE_CYCLES, RENT_EXEMPTION_THRESHOLD},
    deposit::sweep::{deposit_sol, deposit_status, sweepable_amount},
    state::{event::EventType, read_state},
    test_fixtures::{
        DEPOSIT_CONSOLIDATION_FEE, EventsAssert, MINIMUM_DEPOSIT_AMOUNT,
        PROCESS_DEPOSIT_REQUIRED_CYCLES, account, deposit::DEPOSITOR_ACCOUNT,
        init_schnorr_master_key, init_state, runtime::TestCanisterRuntime,
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
            Some(DepositSolStatus::Queued {
                sweepable_amount: balance - RENT_EXEMPTION_THRESHOLD
            })
        );
        assert_eq!(
            runtime.msg_cycles_accepted(),
            [GET_BALANCE_CYCLES - GET_BALANCE_REFUND + DEPOSIT_CONSOLIDATION_FEE]
        );
    }
    assert_eq!(deposit_status(2), None);
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

    let runtime =
        TestCanisterRuntime::new().add_msg_cycles_available(PROCESS_DEPOSIT_REQUIRED_CYCLES);
    let result = deposit_sol(&runtime, second).await;

    assert_eq!(result, Ok(deposit_id));
    assert!(runtime.msg_cycles_accepted().is_empty());
    assert_eq!(
        read_state(|state| state.in_flight_deposit_id(&second)),
        Some(deposit_id)
    );
    EventsAssert::from_recorded()
        .expect_event_eq(queued_deposit_event(
            deposit_id,
            first,
            MINIMUM_DEPOSIT_AMOUNT - RENT_EXEMPTION_THRESHOLD,
        ))
        .assert_no_more_events();
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
