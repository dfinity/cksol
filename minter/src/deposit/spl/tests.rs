use super::*;
use crate::{
    state::{State, audit::process_event, event::EventType, reset_state},
    storage::reset_events,
    test_fixtures::{
        EventsAssert, account, init_schnorr_master_key, init_state,
        runtime::{TEST_CANISTER_ID, TestCanisterRuntime},
        schnorr_master_key, spl_token_account_info,
    },
};
use candid::Principal;
use cksol_types_internal::{SplTokenRegistration, SplTokenUpdate, UpgradeArgs};
use futures::FutureExt;
use ic_canister_runtime::IcError;
use sol_rpc_types::{GetAccountInfoParams, MultiRpcResult, RpcConfig, RpcSources};
use std::panic::AssertUnwindSafe;

const FEE: u64 = 10;
const MINIMUM_NET: u64 = 100;
const CONSOLIDATION: u128 = 70_000_000_000;
const RPC_COST: u128 = 3_000_000_000;

fn mint() -> Address {
    [2; 32].into()
}
fn source_owner(owner: &Account) -> Address {
    account_address(&schnorr_master_key(), owner)
}

fn upgrade(args: UpgradeArgs) {
    let runtime = TestCanisterRuntime::new().add_times([0, 0]);
    mutate_state(|s| process_event(s, EventType::Upgrade(args), &runtime));
}

fn registration(mint: Address, ledger_id: u8) -> SplTokenRegistration {
    SplTokenRegistration {
        mint: mint.to_string(),
        token_program: spl_token_interface::id().to_string(),
        decimals: 6,
        ledger_id: Principal::from_slice(&[ledger_id, 1]),
        index_id: None,
        symbol: "ckTest".into(),
        minimum_deposit_amount: MINIMUM_NET,
        deposit_fee: FEE,
        deposits_paused: false,
    }
}

fn setup_without_key() {
    reset_state();
    init_state();
    upgrade(UpgradeArgs {
        add_spl_tokens: Some(vec![registration(mint(), 99)]),
        ..Default::default()
    });
    reset_events();
}

fn setup() {
    setup_without_key();
    init_schnorr_master_key();
}

fn runtime(owner: Account, token: Address, balance: u64) -> TestCanisterRuntime {
    TestCanisterRuntime::new()
        .expecting_charges()
        .add_times([1_000; 20])
        .add_msg_cycles_available(GET_ACCOUNT_INFO_CYCLES + CONSOLIDATION)
        .add_msg_cycles_refunded(GET_ACCOUNT_INFO_CYCLES - RPC_COST)
        .add_stub_response(MultiRpcResult::Consistent(Ok(Some(
            spl_token_account_info(source_owner(&owner), token, balance),
        ))))
}

fn update(minimum: Option<u64>, fee: Option<u64>, paused: Option<bool>) -> UpgradeArgs {
    UpgradeArgs {
        update_spl_tokens: Some(vec![SplTokenUpdate {
            mint: mint().to_string(),
            minimum_deposit_amount: minimum,
            deposit_fee: fee,
            deposits_paused: paused,
        }]),
        ..Default::default()
    }
}

#[tokio::test]
async fn should_queue_one_deposit_and_charge_only_one_source_read_plus_consolidation() {
    setup();
    let owner = account(1);
    let runtime = runtime(owner, mint(), 1_000);
    assert_eq!(deposit_spl(&runtime, owner, mint().into()).await, Ok(0));
    let calls = runtime.sent_update_calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].method, "getAccountInfo");
    let (_, _, params): (RpcSources, Option<RpcConfig>, GetAccountInfoParams) = calls[0].args();
    assert_eq!(
        params.pubkey,
        associated_token_address(&source_owner(&owner), &mint()).into()
    );
    assert_eq!(runtime.msg_cycles_accepted(), vec![RPC_COST, CONSOLIDATION]);
    EventsAssert::from_recorded()
        .expect_event_eq(EventType::QueuedSplDeposit {
            deposit_id: 0,
            account: owner,
            mint: mint(),
            address: associated_token_address(&source_owner(&owner), &mint()),
            balance: 1_000,
            fee: FEE,
        })
        .assert_no_more_events();
    read_state(|state| {
        assert_eq!(state.spl_deposits().queued()[&0].amount_to_mint(), 990);
        assert_eq!(state.spl_deposits().in_flight_id(&owner, &mint()), Some(0));
        assert_eq!(state.balance(), 0);
        assert_eq!(state.operating_balance().balance(), 0);
        assert!(state.deposits().queued().is_empty());
    });
}

#[tokio::test]
async fn should_reject_unsupported_and_paused_mints_before_calls_or_charges() {
    setup();
    let owner = account(1);
    let unsupported = Address::from([3; 32]);
    let runtime = TestCanisterRuntime::new();
    assert_eq!(
        deposit_spl(&runtime, owner, unsupported.into()).await,
        Err(DepositSplError::UnsupportedToken {
            mint: unsupported.into()
        })
    );
    upgrade(update(None, None, Some(true)));
    reset_events();
    assert_eq!(
        deposit_spl(&runtime, owner, mint().into()).await,
        Err(DepositSplError::TokenPaused {
            mint: mint().into()
        })
    );
    assert!(runtime.sent_update_calls().is_empty());
    assert!(runtime.msg_cycles_accepted().is_empty());
    EventsAssert::assert_no_events_recorded();
}

#[tokio::test]
async fn should_reject_insufficient_cycles_before_reading_the_source() {
    setup();
    let required = read_state(State::deposit_spl_required_cycles);
    let runtime = TestCanisterRuntime::new().add_msg_cycles_available(required - 1);
    assert_eq!(
        deposit_spl(&runtime, account(1), mint().into()).await,
        Err(DepositSplError::InsufficientCycles(
            cksol_types::InsufficientCyclesError {
                expected: required,
                received: required - 1
            }
        ))
    );
    assert!(runtime.sent_update_calls().is_empty());
    EventsAssert::assert_no_events_recorded();
    // The rejected request releases its pair guard.
    let retry = self::runtime(account(1), mint(), MINIMUM_NET + FEE);
    assert_eq!(deposit_spl(&retry, account(1), mint().into()).await, Ok(0));
}

#[tokio::test]
async fn should_reject_unsafe_owners_without_calls_or_charges() {
    setup();
    for principal in [Principal::anonymous(), TEST_CANISTER_ID] {
        let runtime = TestCanisterRuntime::new();
        let owner = Account {
            owner: principal,
            subaccount: None,
        };
        assert!(
            AssertUnwindSafe(deposit_spl(&runtime, owner, mint().into()))
                .catch_unwind()
                .await
                .is_err()
        );
        assert!(runtime.sent_update_calls().is_empty());
        assert!(runtime.msg_cycles_accepted().is_empty());
    }
    EventsAssert::assert_no_events_recorded();
}

#[tokio::test]
async fn should_wait_for_the_master_key_without_reading_or_charging() {
    setup_without_key();
    let runtime = TestCanisterRuntime::new()
        .add_msg_cycles_available(GET_ACCOUNT_INFO_CYCLES + CONSOLIDATION);
    assert!(matches!(
        deposit_spl(&runtime, account(1), mint().into()).await,
        Err(DepositSplError::TemporarilyUnavailable(_))
    ));
    assert!(runtime.sent_update_calls().is_empty());
    assert!(runtime.msg_cycles_accepted().is_empty());
}

#[tokio::test]
async fn should_accept_exactly_the_minimum_gross_balance_and_reject_smaller_balances() {
    for balance in [0, MINIMUM_NET + FEE - 1, MINIMUM_NET + FEE] {
        setup();
        let runtime = runtime(account(1), mint(), balance);
        let result = deposit_spl(&runtime, account(1), mint().into()).await;
        if balance == MINIMUM_NET + FEE {
            assert_eq!(result, Ok(0));
            assert_eq!(runtime.msg_cycles_accepted(), vec![RPC_COST, CONSOLIDATION]);
        } else {
            assert_eq!(
                result,
                Err(DepositSplError::ValueTooSmall {
                    balance,
                    minimum_balance: MINIMUM_NET + FEE
                })
            );
            assert_eq!(runtime.msg_cycles_accepted(), vec![RPC_COST]);
            EventsAssert::assert_no_events_recorded();
        }
    }
}

#[tokio::test]
async fn should_charge_only_actual_rpc_cost_on_a_call_failure() {
    setup();
    let runtime = TestCanisterRuntime::new()
        .expecting_charges()
        .add_msg_cycles_available(GET_ACCOUNT_INFO_CYCLES + CONSOLIDATION)
        .add_msg_cycles_refunded(GET_ACCOUNT_INFO_CYCLES - RPC_COST)
        .add_stub_error(IcError::CallPerformFailed);
    let before = read_state(Clone::clone);
    assert!(matches!(
        deposit_spl(&runtime, account(1), mint().into()).await,
        Err(DepositSplError::TemporarilyUnavailable(_))
    ));
    assert_eq!(runtime.msg_cycles_accepted(), vec![RPC_COST]);
    read_state(|s| assert_eq!(s, &before));
    EventsAssert::assert_no_events_recorded();
}

#[tokio::test]
async fn should_recheck_a_pause_after_the_read() {
    setup();
    let runtime = runtime(account(1), mint(), 1_000);
    let change = async {
        upgrade(update(None, None, Some(true)));
    };
    let (result, ()) = tokio::join!(deposit_spl(&runtime, account(1), mint().into()), change);
    assert_eq!(
        result,
        Err(DepositSplError::TokenPaused {
            mint: mint().into()
        })
    );
    assert_eq!(runtime.msg_cycles_accepted(), vec![RPC_COST]);
    assert!(read_state(|s| s.spl_deposits().queued().is_empty()));
}

#[tokio::test]
async fn should_recheck_the_minimum_and_token_fee_after_the_read() {
    for change in [
        update(Some(1_000), None, None),
        update(None, Some(200), None),
    ] {
        setup();
        let runtime = runtime(account(1), mint(), 200);
        let configure = async {
            upgrade(change);
        };
        let (result, ()) =
            tokio::join!(deposit_spl(&runtime, account(1), mint().into()), configure);
        assert!(matches!(result, Err(DepositSplError::ValueTooSmall { .. })));
        assert_eq!(runtime.msg_cycles_accepted(), vec![RPC_COST]);
        assert!(read_state(|s| s.spl_deposits().queued().is_empty()));
    }
}

#[tokio::test]
async fn should_capture_the_current_token_fee_at_queueing() {
    setup();
    let runtime = runtime(account(1), mint(), 1_000);
    let configure = async {
        upgrade(update(None, Some(20), None));
    };
    let (result, ()) = tokio::join!(deposit_spl(&runtime, account(1), mint().into()), configure);
    assert_eq!(result, Ok(0));
    assert_eq!(read_state(|s| s.spl_deposits().queued()[&0].fee), 20);
}

#[tokio::test]
async fn should_charge_the_consolidation_fee_quoted_before_the_read() {
    setup();
    let runtime = runtime(account(1), mint(), 1_000);
    let configure = async {
        upgrade(UpgradeArgs {
            spl_deposit_consolidation_fee: Some(90_000_000_000),
            ..Default::default()
        });
    };
    let (result, ()) = tokio::join!(deposit_spl(&runtime, account(1), mint().into()), configure);
    assert_eq!(result, Ok(0));
    assert_eq!(runtime.msg_cycles_accepted(), vec![RPC_COST, CONSOLIDATION]);
}

#[tokio::test]
async fn should_return_an_existing_deposit_even_when_the_token_is_paused() {
    setup();
    let first = runtime(account(1), mint(), 1_000);
    assert_eq!(deposit_spl(&first, account(1), mint().into()).await, Ok(0));
    upgrade(update(None, None, Some(true)));
    reset_events();
    let runtime = TestCanisterRuntime::new();
    assert_eq!(
        deposit_spl(&runtime, account(1), mint().into()).await,
        Ok(0)
    );
    assert!(runtime.sent_update_calls().is_empty());
    assert!(runtime.msg_cycles_accepted().is_empty());
    EventsAssert::assert_no_events_recorded();
}

#[tokio::test]
async fn should_reject_concurrent_requests_for_the_same_pair() {
    setup();
    let runtime = runtime(account(1), mint(), 1_000);
    let (first, second) = tokio::join!(
        deposit_spl(&runtime, account(1), mint().into()),
        deposit_spl(
            &runtime,
            Account {
                subaccount: Some([0; 32]),
                ..account(1)
            },
            mint().into()
        )
    );
    assert_eq!(first, Ok(0));
    assert_eq!(second, Err(DepositSplError::AlreadyProcessing));
    assert_eq!(runtime.sent_update_calls().len(), 1);
    assert_eq!(read_state(|s| s.spl_deposits().queued().len()), 1);
}

#[tokio::test]
async fn should_allow_one_owner_to_deposit_distinct_mints_concurrently() {
    setup();
    let other = Address::from([3; 32]);
    upgrade(UpgradeArgs {
        add_spl_tokens: Some(vec![registration(other, 98)]),
        ..Default::default()
    });
    let first = runtime(account(1), mint(), 1_000);
    let second = runtime(account(1), other, 1_000);
    let (a, b) = tokio::join!(
        deposit_spl(&first, account(1), mint().into()),
        deposit_spl(&second, account(1), other.into())
    );
    assert!(a.is_ok() && b.is_ok());
    assert_ne!(a, b);
    assert_eq!(read_state(|s| s.spl_deposits().queued().len()), 2);
}

#[test]
fn should_update_spl_cycle_fees_without_changing_sol_fees() {
    setup();
    let sol_fee = read_state(State::deposit_sol_fee);
    for fee in [0, u64::MAX] {
        upgrade(UpgradeArgs {
            spl_deposit_consolidation_fee: Some(fee),
            ..Default::default()
        });
        assert_eq!(
            read_state(State::spl_deposit_consolidation_fee),
            u128::from(fee)
        );
        assert_eq!(
            read_state(State::deposit_spl_required_cycles),
            GET_ACCOUNT_INFO_CYCLES + u128::from(fee)
        );
        assert_eq!(read_state(State::deposit_sol_fee), sol_fee);
    }
}

#[tokio::test]
async fn should_not_charge_consolidation_or_queue_an_invalid_source_account() {
    setup();
    let mut invalid = spl_token_account_info(source_owner(&account(1)), mint(), 1_000);
    invalid.executable = true;
    let runtime = TestCanisterRuntime::new()
        .expecting_charges()
        .add_msg_cycles_available(GET_ACCOUNT_INFO_CYCLES + CONSOLIDATION)
        .add_msg_cycles_refunded(GET_ACCOUNT_INFO_CYCLES - RPC_COST)
        .add_stub_response(MultiRpcResult::Consistent(Ok(Some(invalid))));
    assert!(matches!(
        deposit_spl(&runtime, account(1), mint().into()).await,
        Err(DepositSplError::InvalidTokenAccount(_))
    ));
    assert_eq!(runtime.msg_cycles_accepted(), vec![RPC_COST]);
    assert!(read_state(|s| s.spl_deposits().queued().is_empty()));
    EventsAssert::assert_no_events_recorded();
}

#[tokio::test]
async fn should_return_the_deposit_queued_during_a_stale_read_without_charging_twice() {
    setup();
    let owner = account(1);
    let runtime = runtime(owner, mint(), 1_000);
    let queue = async {
        let runtime = TestCanisterRuntime::new().add_times([100, 100]);
        mutate_state(|s| {
            process_event(
                s,
                EventType::QueuedSplDeposit {
                    deposit_id: 0,
                    account: owner,
                    mint: mint(),
                    address: associated_token_address(&source_owner(&owner), &mint()),
                    balance: 500,
                    fee: FEE,
                },
                &runtime,
            )
        });
    };
    let (result, ()) = tokio::join!(deposit_spl(&runtime, owner, mint().into()), queue);
    assert_eq!(result, Ok(0));
    assert_eq!(runtime.msg_cycles_accepted(), vec![RPC_COST]);
    assert_eq!(read_state(|s| s.spl_deposits().queued()[&0].balance), 500);
    assert_eq!(read_state(|s| s.spl_deposits().next_id()), 1);
}

#[tokio::test]
async fn should_bound_concurrent_spl_requests_and_release_capacity() {
    setup();
    let guards: Vec<_> = (1..=100)
        .map(|id| deposit_spl_guard(account(id), mint()).unwrap())
        .collect();
    let full = TestCanisterRuntime::new();
    assert!(matches!(
        deposit_spl(&full, account(101), mint().into()).await,
        Err(DepositSplError::TemporarilyUnavailable(_))
    ));
    assert!(full.sent_update_calls().is_empty());
    drop(guards);
    let runtime = runtime(account(101), mint(), 1_000);
    assert_eq!(
        deposit_spl(&runtime, account(101), mint().into()).await,
        Ok(0)
    );
}
