use crate::{
    state::event::{DepositId, EventType},
    test_fixtures::{
        BLOCK_INDEX, DEPOSIT_CONSOLIDATION_FEE, DEPOSIT_FEE, EventsAssert,
        UPDATE_BALANCE_REQUIRED_CYCLES,
        deposit::{
            DEPOSIT_AMOUNT, DEPOSITOR_ACCOUNT, DEPOSITOR_PRINCIPAL, accepted_deposit_event,
            deposit_status_minted, deposit_status_processing, deposit_status_quarantined,
            deposit_transaction, deposit_transaction_signature,
            deposit_transaction_to_multiple_accounts,
            deposit_transaction_to_multiple_accounts_signature,
            deposit_transaction_to_wrong_address, deposit_transaction_to_wrong_address_signature,
            minted_event, quarantined_deposit_event,
        },
        init_schnorr_master_key, init_state, init_state_with_args,
        runtime::TestCanisterRuntime,
        valid_init_args,
    },
    update_balance::update_balance,
};
use assert_matches::assert_matches;
use candid_parser::Principal;
use cksol_types::{DepositStatus, InsufficientCyclesError, UpdateBalanceError};
use cksol_types_internal::InitArgs;
use ic_canister_runtime::IcError;
use icrc_ledger_types::icrc1::{
    account::Account,
    transfer::{BlockIndex, TransferError},
};
use sol_rpc_types::{EncodedConfirmedTransactionWithStatusMeta, Lamport, MultiRpcResult};
use std::panic;

type GetTransactionResult = MultiRpcResult<Option<EncodedConfirmedTransactionWithStatusMeta>>;

#[tokio::test]
async fn should_fail_if_insufficient_cycles_attached() {
    init_state();

    let runtime =
        TestCanisterRuntime::new().add_msg_cycles_available(UPDATE_BALANCE_REQUIRED_CYCLES - 1);

    let result = update_balance(runtime, DEPOSITOR_ACCOUNT, deposit_transaction_signature()).await;

    assert_eq!(
        result,
        Err(UpdateBalanceError::InsufficientCycles(
            InsufficientCyclesError {
                expected: UPDATE_BALANCE_REQUIRED_CYCLES,
                received: UPDATE_BALANCE_REQUIRED_CYCLES - 1,
            }
        ))
    );
    EventsAssert::assert_no_events_recorded();
}

#[tokio::test]
async fn should_return_error_if_get_transaction_fails() {
    init_state();
    init_schnorr_master_key();

    let runtime = runtime_with_time_and_cycles().add_stub_error(IcError::CallPerformFailed);

    let result = update_balance(runtime, DEPOSITOR_ACCOUNT, deposit_transaction_signature()).await;

    assert_matches!(
        result,
        Err(UpdateBalanceError::TemporarilyUnavailable(e)) => assert!(e.contains("Inter-canister call perform failed"))
    );
    EventsAssert::assert_no_events_recorded();
}

#[tokio::test]
async fn should_return_error_if_transaction_not_found() {
    init_state();
    init_schnorr_master_key();

    let runtime = runtime_with_time_and_cycles()
        .add_stub_response(GetTransactionResult::Consistent(Ok(None)));

    let result = update_balance(runtime, DEPOSITOR_ACCOUNT, deposit_transaction_signature()).await;

    assert_eq!(result, Err(UpdateBalanceError::TransactionNotFound));
    EventsAssert::assert_no_events_recorded();
}

#[tokio::test]
async fn should_return_error_if_transaction_not_valid_deposit() {
    init_state();
    init_schnorr_master_key();

    let get_transaction_response = GetTransactionResult::Consistent(Ok(Some(
        deposit_transaction_to_wrong_address().try_into().unwrap(),
    )));
    let runtime = runtime_with_time_and_cycles().add_stub_response(get_transaction_response);

    let result = update_balance(
        runtime,
        DEPOSITOR_ACCOUNT,
        deposit_transaction_to_wrong_address_signature(),
    )
    .await;

    assert_matches!(
        result,
        Err(UpdateBalanceError::InvalidDepositTransaction(e)) => assert!(e.contains("Transaction must target deposit address"))
    );
    EventsAssert::assert_no_events_recorded();
}

#[tokio::test]
async fn should_fail_if_deposit_amount_is_below_minimum() {
    const MINIMUM_DEPOSIT_AMOUNT: Lamport = 2 * DEPOSIT_AMOUNT;
    init_state_with_args(InitArgs {
        minimum_deposit_amount: MINIMUM_DEPOSIT_AMOUNT,
        ..valid_init_args()
    });
    init_schnorr_master_key();

    let get_transaction_response =
        GetTransactionResult::Consistent(Ok(Some(deposit_transaction().try_into().unwrap())));
    let runtime = runtime_with_time_and_cycles().add_stub_response(get_transaction_response);

    let result = update_balance(runtime, DEPOSITOR_ACCOUNT, deposit_transaction_signature()).await;

    assert_eq!(
        result,
        Err(UpdateBalanceError::ValueTooSmall {
            deposit_amount: DEPOSIT_AMOUNT,
            minimum_deposit_amount: MINIMUM_DEPOSIT_AMOUNT,
        })
    );
    EventsAssert::assert_no_events_recorded();
}

#[tokio::test]
async fn should_return_processing_if_mint_fails() {
    init_state();
    init_schnorr_master_key();

    let get_transaction_response =
        GetTransactionResult::Consistent(Ok(Some(deposit_transaction().try_into().unwrap())));
    let runtime = runtime_with_time_and_cycles()
        .add_stub_response(get_transaction_response)
        .add_stub_response(Err::<BlockIndex, TransferError>(
            TransferError::TemporarilyUnavailable,
        ));

    let result = update_balance(runtime, DEPOSITOR_ACCOUNT, deposit_transaction_signature()).await;

    assert_eq!(result, Ok(deposit_status_processing()));

    EventsAssert::from_recorded()
        .expect_event_eq(accepted_deposit_event())
        .assert_no_more_events();
}

#[tokio::test]
async fn should_successfully_mint_on_second_call() {
    init_state();
    init_schnorr_master_key();

    // First call: makes JSON-RPC call and attempts to mint
    let get_transaction_response =
        GetTransactionResult::Consistent(Ok(Some(deposit_transaction().try_into().unwrap())));
    let runtime = runtime_with_time_and_cycles()
        .add_stub_response(get_transaction_response)
        .add_stub_response(Err::<BlockIndex, TransferError>(
            TransferError::TemporarilyUnavailable,
        ));
    let result = update_balance(runtime, DEPOSITOR_ACCOUNT, deposit_transaction_signature()).await;
    assert_eq!(result, Ok(deposit_status_processing()));

    // Second call: fetches status from minter state, and mints successfully without making any
    // additional JSON-RPC calls
    let runtime = TestCanisterRuntime::new()
        .with_increasing_time()
        .add_stub_response(Ok::<BlockIndex, TransferError>(BLOCK_INDEX.into()));
    let result = update_balance(runtime, DEPOSITOR_ACCOUNT, deposit_transaction_signature()).await;
    assert_eq!(result, Ok(deposit_status_minted()));

    EventsAssert::from_recorded()
        .expect_event_eq(accepted_deposit_event())
        .expect_event_eq(minted_event(BLOCK_INDEX))
        .assert_no_more_events();
}

#[tokio::test]
async fn should_succeed_with_valid_deposit_transaction() {
    init_state();
    init_schnorr_master_key();

    let get_transaction_response =
        GetTransactionResult::Consistent(Ok(Some(deposit_transaction().try_into().unwrap())));
    let runtime = runtime_with_time_and_cycles()
        .add_stub_response(get_transaction_response)
        .add_stub_response(Ok::<BlockIndex, TransferError>(BLOCK_INDEX.into()));

    let result = update_balance(runtime, DEPOSITOR_ACCOUNT, deposit_transaction_signature()).await;

    assert_eq!(result, Ok(deposit_status_minted()));

    EventsAssert::from_recorded()
        .expect_event_eq(accepted_deposit_event())
        .expect_event_eq(minted_event(BLOCK_INDEX))
        .assert_no_more_events();
}

#[tokio::test]
async fn should_not_double_mint() {
    init_state();
    init_schnorr_master_key();

    // Successful mint
    let get_transaction_response =
        GetTransactionResult::Consistent(Ok(Some(deposit_transaction().try_into().unwrap())));
    let runtime = runtime_with_time_and_cycles()
        .add_stub_response(get_transaction_response)
        .add_stub_response(Ok::<BlockIndex, TransferError>(BLOCK_INDEX.into()));
    let result = update_balance(runtime, DEPOSITOR_ACCOUNT, deposit_transaction_signature()).await;
    assert_eq!(result, Ok(deposit_status_minted()));

    // Second call: returns the same status
    let runtime = TestCanisterRuntime::new();
    let result = update_balance(runtime, DEPOSITOR_ACCOUNT, deposit_transaction_signature()).await;
    assert_eq!(result, Ok(deposit_status_minted()));

    // Only one mint event recorded
    EventsAssert::from_recorded()
        .expect_event_eq(accepted_deposit_event())
        .expect_event_eq(minted_event(BLOCK_INDEX))
        .assert_no_more_events();
}

#[tokio::test]
async fn should_quarantine_deposit() {
    init_state();
    init_schnorr_master_key();

    // Don't mock the ledger response so the runtime panics when calling it to mint
    let get_transaction_response =
        GetTransactionResult::Consistent(Ok(Some(deposit_transaction().try_into().unwrap())));
    let runtime = || runtime_with_time_and_cycles().add_stub_response(get_transaction_response);
    let first_result = tokio::spawn(async move {
        update_balance(
            runtime(),
            DEPOSITOR_ACCOUNT,
            deposit_transaction_signature(),
        )
        .await
    })
    .await;
    assert!(first_result.is_err_and(|e| e.is_panic()));

    // On the second call, the deposit should have been quarantined
    let runtime = TestCanisterRuntime::new();
    let second_result =
        update_balance(runtime, DEPOSITOR_ACCOUNT, deposit_transaction_signature()).await;
    assert_eq!(second_result, Ok(deposit_status_quarantined()));

    // Calling `update_balance` again for the same deposit should return the same status
    let runtime = TestCanisterRuntime::new();
    let third_result =
        update_balance(runtime, DEPOSITOR_ACCOUNT, deposit_transaction_signature()).await;
    assert_eq!(third_result, second_result);

    // Only one mint event recorded
    EventsAssert::from_recorded()
        .expect_event_eq(accepted_deposit_event())
        .expect_event_eq(quarantined_deposit_event())
        .assert_no_more_events();
}

#[tokio::test]
async fn should_allow_deposits_to_multiple_accounts_with_single_transaction() {
    const ACCOUNTS: [Account; 3] = [
        Account {
            owner: DEPOSITOR_PRINCIPAL,
            subaccount: None,
        },
        Account {
            owner: DEPOSITOR_PRINCIPAL,
            subaccount: Some([1; 32]),
        },
        Account {
            owner: Principal::from_slice(&[0xa; 29]),
            subaccount: Some([2; 32]),
        },
    ];
    const DEPOSIT_AMOUNTS: [Lamport; 3] = [
        100_000_000, // 0.1 SOL
        200_000_000, // 0.2 SOL
        300_000_000, // 0.3 SOL
    ];
    const BLOCK_INDEXES: [u64; 3] = [79853, 79854, 79855];

    init_state();
    init_schnorr_master_key();

    let get_transaction_response = GetTransactionResult::Consistent(Ok(Some(
        deposit_transaction_to_multiple_accounts()
            .try_into()
            .unwrap(),
    )));

    for i in 0..3 {
        let runtime = runtime_with_time_and_cycles()
            .add_stub_response(get_transaction_response.clone())
            .add_stub_response(Ok::<BlockIndex, TransferError>(BLOCK_INDEXES[i].into()));
        let result = update_balance(
            runtime,
            ACCOUNTS[i],
            deposit_transaction_to_multiple_accounts_signature(),
        )
        .await;
        assert_eq!(
            result,
            Ok(DepositStatus::Minted {
                block_index: BLOCK_INDEXES[i],
                minted_amount: DEPOSIT_AMOUNTS[i] - DEPOSIT_FEE,
                deposit_id: cksol_types::DepositId {
                    signature: deposit_transaction_to_multiple_accounts_signature().into(),
                    account: ACCOUNTS[i],
                },
            })
        );
    }

    let mut events_assert = EventsAssert::from_recorded();
    for i in 0..3 {
        let deposit_id = DepositId {
            signature: deposit_transaction_to_multiple_accounts_signature(),
            account: ACCOUNTS[i],
        };
        events_assert = events_assert
            .expect_event_eq(EventType::AcceptedManualDeposit {
                deposit_id,
                deposit_amount: DEPOSIT_AMOUNTS[i],
                amount_to_mint: DEPOSIT_AMOUNTS[i] - DEPOSIT_FEE,
            })
            .expect_event_eq(EventType::Minted {
                deposit_id,
                mint_block_index: BLOCK_INDEXES[i].into(),
            })
    }
    events_assert.assert_no_more_events();
}

fn runtime_with_time_and_cycles() -> TestCanisterRuntime {
    // Cycles forwarded to the RPC call = total - consolidation fee
    let cycles_for_rpc = UPDATE_BALANCE_REQUIRED_CYCLES - DEPOSIT_CONSOLIDATION_FEE;
    // Simulate the RPC canister refunding most of the forwarded cycles
    let refunded: u128 = cycles_for_rpc - 100_000_000_000;
    let rpc_cost = cycles_for_rpc - refunded;
    TestCanisterRuntime::new()
        .with_increasing_time()
        .add_msg_cycles_available(UPDATE_BALANCE_REQUIRED_CYCLES)
        .add_msg_cycles_accept(rpc_cost + DEPOSIT_CONSOLIDATION_FEE)
        .add_msg_cycles_refunded(refunded)
}
