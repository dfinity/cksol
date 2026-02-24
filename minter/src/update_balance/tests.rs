use crate::{
    runtime::TestCanisterRuntime,
    state::event::EventType,
    test_fixtures::{
        BLOCK_INDEX, EventsAssert,
        deposit::{
            DEPOSIT_AMOUNT, DEPOSITOR_ACCOUNT, accepted_deposit_event, deposit_status_minted,
            deposit_status_processing, deposit_transaction, deposit_transaction_signature,
            deposit_transaction_to_wrong_address, deposit_transaction_to_wrong_address_signature,
            minted_event,
        },
        init_schnorr_master_key, init_state, init_state_with_args, valid_init_args,
    },
    update_balance::update_balance,
};
use assert_matches::assert_matches;
use cksol_types::UpdateBalanceError;
use cksol_types_internal::InitArgs;
use ic_canister_runtime::IcError;
use icrc_ledger_types::icrc1::transfer::{BlockIndex, TransferError};
use sol_rpc_types::{EncodedConfirmedTransactionWithStatusMeta, MultiRpcResult};

type GetTransactionResult = MultiRpcResult<Option<EncodedConfirmedTransactionWithStatusMeta>>;

#[tokio::test]
async fn should_return_error_if_get_transaction_fails() {
    init_state();
    init_schnorr_master_key();

    let runtime = TestCanisterRuntime::new().add_stub_error(IcError::CallPerformFailed);

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

    let runtime =
        TestCanisterRuntime::new().add_stub_response(GetTransactionResult::Consistent(Ok(None)));

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
    let runtime = TestCanisterRuntime::new().add_stub_response(get_transaction_response);

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
async fn should_fail_if_deposit_too_small() {
    init_state_with_args(InitArgs {
        deposit_fee: 2 * DEPOSIT_AMOUNT,
        ..valid_init_args()
    });
    init_schnorr_master_key();

    let get_transaction_response =
        GetTransactionResult::Consistent(Ok(Some(deposit_transaction().try_into().unwrap())));
    let runtime = TestCanisterRuntime::new().add_stub_response(get_transaction_response);

    let result = update_balance(runtime, DEPOSITOR_ACCOUNT, deposit_transaction_signature()).await;

    assert_eq!(result, Err(UpdateBalanceError::ValueTooSmall));
    EventsAssert::assert_no_events_recorded();
}

#[tokio::test]
async fn should_return_processing_if_mint_fails() {
    init_state();
    init_schnorr_master_key();

    let get_transaction_response =
        GetTransactionResult::Consistent(Ok(Some(deposit_transaction().try_into().unwrap())));
    let runtime = TestCanisterRuntime::new()
        .with_increasing_time()
        .add_stub_response(get_transaction_response)
        .add_stub_response(Err::<BlockIndex, TransferError>(
            TransferError::TemporarilyUnavailable,
        ));

    let result = update_balance(runtime, DEPOSITOR_ACCOUNT, deposit_transaction_signature()).await;

    assert_eq!(result, Ok(deposit_status_processing()));

    EventsAssert::from_recorded()
        .expect_event_eq(EventType::AcceptedDeposit(accepted_deposit_event()))
        .assert_no_more_events();
}

#[tokio::test]
async fn should_return_processing_again_on_second_call() {
    init_state();
    init_schnorr_master_key();

    // First call: makes JSON-RPC call and attempts to mint
    let get_transaction_response =
        GetTransactionResult::Consistent(Ok(Some(deposit_transaction().try_into().unwrap())));
    let runtime = TestCanisterRuntime::new()
        .with_increasing_time()
        .add_stub_response(get_transaction_response)
        .add_stub_response(Err::<BlockIndex, TransferError>(
            TransferError::TemporarilyUnavailable,
        ));
    let result = update_balance(runtime, DEPOSITOR_ACCOUNT, deposit_transaction_signature()).await;
    assert_eq!(result, Ok(deposit_status_processing()));

    // Second call: fetches status from minter state, no JSON-RPC or minter calls
    let runtime = TestCanisterRuntime::new();
    let result = update_balance(runtime, DEPOSITOR_ACCOUNT, deposit_transaction_signature()).await;
    assert_eq!(result, Ok(deposit_status_processing()));

    EventsAssert::from_recorded()
        .expect_event_eq(EventType::AcceptedDeposit(accepted_deposit_event()))
        .assert_no_more_events();
}

#[tokio::test]
async fn should_succeed_with_valid_deposit_transaction() {
    const BLOCK_INDEX: u64 = 98763_u64;

    init_state();
    init_schnorr_master_key();

    let get_transaction_response =
        GetTransactionResult::Consistent(Ok(Some(deposit_transaction().try_into().unwrap())));
    let runtime = TestCanisterRuntime::new()
        .with_increasing_time()
        .add_stub_response(get_transaction_response)
        .add_stub_response(Ok::<BlockIndex, TransferError>(BLOCK_INDEX.into()));

    let result = update_balance(runtime, DEPOSITOR_ACCOUNT, deposit_transaction_signature()).await;

    assert_eq!(result, Ok(deposit_status_minted()));

    EventsAssert::from_recorded()
        .expect_event_eq(EventType::AcceptedDeposit(accepted_deposit_event()))
        .expect_event_eq(EventType::Minted(minted_event(BLOCK_INDEX)))
        .assert_no_more_events();
}

#[tokio::test]
async fn should_not_double_mint() {
    init_state();
    init_schnorr_master_key();

    // Successful mint
    let get_transaction_response =
        GetTransactionResult::Consistent(Ok(Some(deposit_transaction().try_into().unwrap())));
    let runtime = TestCanisterRuntime::new()
        .with_increasing_time()
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
        .expect_event_eq(EventType::AcceptedDeposit(accepted_deposit_event()))
        .expect_event_eq(EventType::Minted(minted_event(BLOCK_INDEX)))
        .assert_no_more_events();
}
