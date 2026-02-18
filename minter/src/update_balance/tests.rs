use crate::{
    test_fixtures::{
        DEPOSIT_FEE,
        deposit::{
            DEPOSIT_AMOUNT, DEPOSITOR_ACCOUNT, deposit_transaction, deposit_transaction_signature,
            deposit_transaction_to_wrong_address, deposit_transaction_to_wrong_address_signature,
        },
        init_schnorr_master_key, init_state,
    },
    update_balance::update_balance,
};
use assert_matches::assert_matches;
use cksol_types::{DepositStatus, UpdateBalanceError};
use ic_canister_runtime::{IcError, StubRuntime};
use icrc_ledger_types::icrc1::transfer::{BlockIndex, TransferError};
use sol_rpc_types::{EncodedConfirmedTransactionWithStatusMeta, MultiRpcResult};

type GetTransactionResult = MultiRpcResult<Option<EncodedConfirmedTransactionWithStatusMeta>>;

#[tokio::test]
async fn should_return_error_if_get_transaction_fails() {
    init_state();
    init_schnorr_master_key();

    let runtime = StubRuntime::new().add_stub_error(IcError::CallPerformFailed);

    let result = update_balance(runtime, DEPOSITOR_ACCOUNT, deposit_transaction_signature()).await;

    assert_matches!(
        result,
        Err(UpdateBalanceError::TemporarilyUnavailable(e)) => assert!(e.contains("Inter-canister call perform failed"))
    );
}

#[tokio::test]
async fn should_return_error_if_transaction_not_found() {
    init_state();
    init_schnorr_master_key();

    let runtime = StubRuntime::new().add_stub_response(GetTransactionResult::Consistent(Ok(None)));

    let result = update_balance(runtime, DEPOSITOR_ACCOUNT, deposit_transaction_signature()).await;

    assert_eq!(result, Err(UpdateBalanceError::TransactionNotFound))
}

#[tokio::test]
async fn should_return_error_if_transaction_not_valid_deposit() {
    init_state();
    init_schnorr_master_key();

    let get_transaction_response = GetTransactionResult::Consistent(Ok(Some(
        deposit_transaction_to_wrong_address().try_into().unwrap(),
    )));
    let runtime = StubRuntime::new().add_stub_response(get_transaction_response);

    let result = update_balance(
        runtime,
        DEPOSITOR_ACCOUNT,
        deposit_transaction_to_wrong_address_signature(),
    )
    .await;

    assert_matches!(
        result,
        Err(UpdateBalanceError::InvalidDepositTransaction(e)) => assert!(e.contains("Deposit address not part of transaction account keys"))
    );
}

#[tokio::test]
async fn should_fail_if_deposit_too_small() {
    init_state();
    init_schnorr_master_key();

    let mut transaction = deposit_transaction();
    transaction.transaction.meta.as_mut().map(|meta| {
        meta.post_balances[1] = meta.pre_balances[1] + 1; // 1 lamport deposit
    });

    let get_transaction_response =
        GetTransactionResult::Consistent(Ok(Some(transaction.try_into().unwrap())));
    let runtime = StubRuntime::new().add_stub_response(get_transaction_response);

    let result = update_balance(runtime, DEPOSITOR_ACCOUNT, deposit_transaction_signature()).await;

    assert_eq!(result, Err(UpdateBalanceError::ValueTooSmall));
}

#[tokio::test]
async fn should_return_processing_if_mint_fails() {
    init_state();
    init_schnorr_master_key();

    let get_transaction_response =
        GetTransactionResult::Consistent(Ok(Some(deposit_transaction().try_into().unwrap())));
    let runtime = StubRuntime::new()
        .add_stub_response(get_transaction_response)
        .add_stub_response(Err::<BlockIndex, TransferError>(
            TransferError::TemporarilyUnavailable,
        ));

    let result = update_balance(runtime, DEPOSITOR_ACCOUNT, deposit_transaction_signature()).await;

    assert_eq!(
        result,
        Ok(DepositStatus::Processing(
            deposit_transaction_signature().into()
        ))
    )
}

#[tokio::test]
async fn should_succeed_with_valid_deposit_transaction() {
    const BLOCK_INDEX: u64 = 98763_u64;

    init_state();
    init_schnorr_master_key();

    let get_transaction_response =
        GetTransactionResult::Consistent(Ok(Some(deposit_transaction().try_into().unwrap())));
    let runtime = StubRuntime::new()
        .add_stub_response(get_transaction_response)
        .add_stub_response(Ok::<BlockIndex, TransferError>(BLOCK_INDEX.into()));

    let result = update_balance(runtime, DEPOSITOR_ACCOUNT, deposit_transaction_signature()).await;

    assert_eq!(
        result,
        Ok(DepositStatus::Minted {
            block_index: BLOCK_INDEX,
            minted_amount: DEPOSIT_AMOUNT - DEPOSIT_FEE,
            signature: deposit_transaction_signature().into(),
        })
    )
}
