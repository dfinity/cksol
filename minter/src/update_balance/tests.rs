use crate::{
    test_fixtures::{
        deposit::{DEPOSITOR_ACCOUNT, deposit_transaction_signature},
        init_schnorr_master_key, init_state,
    },
    update_balance::update_balance,
};
use assert_matches::assert_matches;
use cksol_types::UpdateBalanceError;
use ic_canister_runtime::{IcError, StubRuntime};
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
