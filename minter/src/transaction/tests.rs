use crate::{
    test_fixtures::{deposit::deposit_transaction_signature, init_state},
    transaction::{GetTransactionError, try_get_transaction},
};
use ic_canister_runtime::{IcError, StubRuntime};
use sol_rpc_types::{HttpOutcallError, RpcError, RpcSource, SupportedRpcProviderId};

mod get_transaction_tests {
    use super::*;

    type MultiRpcResult = sol_rpc_types::MultiRpcResult<
        Option<sol_rpc_types::EncodedConfirmedTransactionWithStatusMeta>,
    >;

    #[tokio::test]
    async fn should_fail_if_get_transaction_fails() {
        init_state();

        let runtime = StubRuntime::new().add_stub_error(IcError::CallPerformFailed);

        let result = try_get_transaction(runtime, deposit_transaction_signature()).await;

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

        let runtime = StubRuntime::new()
            .add_stub_response(MultiRpcResult::Consistent(Err(rpc_error.clone())));

        let result = try_get_transaction(runtime, deposit_transaction_signature()).await;

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

        let runtime = StubRuntime::new().add_stub_response(MultiRpcResult::Inconsistent(results));

        let result = try_get_transaction(runtime, deposit_transaction_signature()).await;

        assert_eq!(result, Err(GetTransactionError::InconsistentRpcResults));
    }

    #[tokio::test]
    async fn should_return_empty_if_transaction_not_found() {
        init_state();

        let runtime = StubRuntime::new().add_stub_response(MultiRpcResult::Consistent(Ok(None)));

        let result = try_get_transaction(runtime, deposit_transaction_signature()).await;

        assert_eq!(result, Ok(None))
    }

    #[tokio::test]
    async fn should_return_transaction() {
        init_state();

        let runtime = StubRuntime::new().add_stub_response(MultiRpcResult::Consistent(Ok(None)));

        let result = try_get_transaction(runtime, deposit_transaction_signature()).await;

        assert_eq!(result, Ok(None))
    }
}
