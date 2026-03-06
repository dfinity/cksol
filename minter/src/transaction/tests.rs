use crate::{
    runtime::TestCanisterRuntime,
    test_fixtures::{
        UPDATE_BALANCE_REQUIRED_CYCLES,
        deposit::{
            DEPOSIT_ADDRESS, DEPOSIT_AMOUNT, deposit_transaction, deposit_transaction_signature,
            deposit_transaction_to_wrong_address,
        },
        init_state,
    },
    transaction::{
        GetDepositAmountError, GetTransactionError, get_deposit_amount_to_address,
        try_get_transaction,
    },
};
use assert_matches::assert_matches;
use ic_canister_runtime::IcError;
use sol_rpc_types::{HttpOutcallError, RpcError, RpcSource, SupportedRpcProviderId};
use solana_transaction_status_client_types::{EncodedTransaction, TransactionBinaryEncoding};

// TODO DEFI-2643: Test behavior with cycles
mod get_transaction_tests {
    use super::*;

    type MultiRpcResult = sol_rpc_types::MultiRpcResult<
        Option<sol_rpc_types::EncodedConfirmedTransactionWithStatusMeta>,
    >;

    #[tokio::test]
    async fn should_fail_if_get_transaction_fails() {
        init_state();

        let runtime = TestCanisterRuntime::new()
            .add_msg_cycles_available(UPDATE_BALANCE_REQUIRED_CYCLES)
            .add_stub_error(IcError::CallPerformFailed);

        let result = try_get_transaction(&runtime, deposit_transaction_signature()).await;

        assert_eq!(
            result,
            Err(GetTransactionError::IcError(IcError::CallPerformFailed))
        );
    }

    #[tokio::test]
    async fn should_fail_if_get_transaction_returns_rpc_error() {
        init_state();

        let rpc_error = RpcError::HttpOutcallError(HttpOutcallError::InvalidHttpJsonRpcResponse {
            status: 500,
            body: "{}}".to_string(),
            parsing_error: None,
        });

        let runtime = TestCanisterRuntime::new()
            .add_msg_cycles_available(UPDATE_BALANCE_REQUIRED_CYCLES)
            .add_stub_response(MultiRpcResult::Consistent(Err(rpc_error.clone())));

        let result = try_get_transaction(&runtime, deposit_transaction_signature()).await;

        assert_eq!(result, Err(GetTransactionError::RpcError(rpc_error)));
    }

    #[tokio::test]
    async fn should_fail_if_get_transaction_result_inconsistent() {
        init_state();

        let results = vec![
            (
                RpcSource::Supported(SupportedRpcProviderId::AnkrMainnet),
                Err(RpcError::ValidationError("Error 1".to_string())),
            ),
            (
                RpcSource::Supported(SupportedRpcProviderId::DrpcMainnet),
                Err(RpcError::ValidationError("Error 2".to_string())),
            ),
        ];

        let runtime = TestCanisterRuntime::new()
            .add_msg_cycles_available(UPDATE_BALANCE_REQUIRED_CYCLES)
            .add_stub_response(MultiRpcResult::Inconsistent(results));

        let result = try_get_transaction(&runtime, deposit_transaction_signature()).await;

        assert_eq!(result, Err(GetTransactionError::InconsistentRpcResults));
    }

    #[tokio::test]
    async fn should_return_empty_if_transaction_not_found() {
        init_state();

        let runtime = TestCanisterRuntime::new()
            .add_msg_cycles_available(UPDATE_BALANCE_REQUIRED_CYCLES)
            .add_stub_response(MultiRpcResult::Consistent(Ok(None)));

        let result = try_get_transaction(&runtime, deposit_transaction_signature()).await;

        assert_eq!(result, Ok(None))
    }

    #[tokio::test]
    async fn should_return_transaction() {
        init_state();

        let runtime = TestCanisterRuntime::new()
            .add_msg_cycles_available(UPDATE_BALANCE_REQUIRED_CYCLES)
            .add_stub_response(MultiRpcResult::Consistent(Ok(Some(
                deposit_transaction().try_into().unwrap(),
            ))));

        let result = try_get_transaction(&runtime, deposit_transaction_signature()).await;

        assert_eq!(result, Ok(Some(deposit_transaction())))
    }
}

mod get_deposit_amount_tests {
    use super::*;

    #[test]
    fn should_fail_if_transaction_decoding_fails() {
        let mut transaction = deposit_transaction();
        transaction.transaction.transaction =
            EncodedTransaction::Binary("invalid".to_string(), TransactionBinaryEncoding::Base64);

        let result = get_deposit_amount_to_address(transaction, DEPOSIT_ADDRESS);

        assert_matches!(
            result,
            Err(GetDepositAmountError::TransactionParsingFailed(e)) => assert!(e.contains("Transaction decoding failed"))
        );
    }

    #[test]
    fn should_fail_if_transaction_has_no_meta() {
        let mut transaction = deposit_transaction();
        transaction.transaction.meta = None;

        let result = get_deposit_amount_to_address(transaction, DEPOSIT_ADDRESS);

        assert_eq!(result, Err(GetDepositAmountError::NoMetaField));
    }

    #[test]
    fn should_fail_if_transaction_deposit_to_wrong_address() {
        let transaction = deposit_transaction_to_wrong_address();

        let result = get_deposit_amount_to_address(transaction, DEPOSIT_ADDRESS);

        assert_eq!(
            result,
            Err(GetDepositAmountError::DepositAddressNotInAccountKeys)
        );
    }

    #[test]
    fn should_succeed_for_valid_deposit() {
        let transaction = deposit_transaction();

        let result = get_deposit_amount_to_address(transaction, DEPOSIT_ADDRESS);

        assert_eq!(result, Ok(DEPOSIT_AMOUNT));
    }
}
