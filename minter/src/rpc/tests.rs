use crate::{
    rpc::{
        GetRecentBlockhashError, GetSignaturesForAddressError, GetTransactionError,
        SubmitTransactionError, get_recent_slot_and_blockhash, get_signatures_for_address,
        get_transaction, submit_transaction,
    },
    test_fixtures::{
        PROCESS_DEPOSIT_REQUIRED_CYCLES, confirmed_block,
        deposit::{legacy_deposit_transaction, legacy_deposit_transaction_signature},
        init_state,
        runtime::TestCanisterRuntime,
    },
};
use assert_matches::assert_matches;
use ic_canister_runtime::IcError;
use sol_rpc_types::{HttpOutcallError, RpcError, RpcSource, SupportedRpcProviderId};
use solana_transaction::{Message, Transaction};

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
            .add_msg_cycles_available(PROCESS_DEPOSIT_REQUIRED_CYCLES)
            .add_stub_error(IcError::CallPerformFailed);

        let result = get_transaction(
            &runtime,
            legacy_deposit_transaction_signature(),
            PROCESS_DEPOSIT_REQUIRED_CYCLES,
        )
        .await;

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
            .add_msg_cycles_available(PROCESS_DEPOSIT_REQUIRED_CYCLES)
            .add_stub_response(MultiRpcResult::Consistent(Err(rpc_error.clone())));

        let result = get_transaction(
            &runtime,
            legacy_deposit_transaction_signature(),
            PROCESS_DEPOSIT_REQUIRED_CYCLES,
        )
        .await;

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
            .add_msg_cycles_available(PROCESS_DEPOSIT_REQUIRED_CYCLES)
            .add_stub_response(MultiRpcResult::Inconsistent(results));

        let result = get_transaction(
            &runtime,
            legacy_deposit_transaction_signature(),
            PROCESS_DEPOSIT_REQUIRED_CYCLES,
        )
        .await;

        assert_eq!(result, Err(GetTransactionError::InconsistentRpcResults));
    }

    #[tokio::test]
    async fn should_return_empty_if_transaction_not_found() {
        init_state();

        let runtime = TestCanisterRuntime::new()
            .add_msg_cycles_available(PROCESS_DEPOSIT_REQUIRED_CYCLES)
            .add_stub_response(MultiRpcResult::Consistent(Ok(None)));

        let result = get_transaction(
            &runtime,
            legacy_deposit_transaction_signature(),
            PROCESS_DEPOSIT_REQUIRED_CYCLES,
        )
        .await;

        assert_eq!(result, Ok(None))
    }

    #[tokio::test]
    async fn should_return_transaction() {
        init_state();

        let runtime = TestCanisterRuntime::new()
            .add_msg_cycles_available(PROCESS_DEPOSIT_REQUIRED_CYCLES)
            .add_stub_response(MultiRpcResult::Consistent(Ok(Some(
                legacy_deposit_transaction().try_into().unwrap(),
            ))));

        let result = get_transaction(
            &runtime,
            legacy_deposit_transaction_signature(),
            PROCESS_DEPOSIT_REQUIRED_CYCLES,
        )
        .await;

        assert_eq!(result, Ok(Some(legacy_deposit_transaction())))
    }
}

mod submit_transaction_tests {
    use super::*;

    type SendTransactionResult = sol_rpc_types::MultiRpcResult<sol_rpc_types::Signature>;

    #[tokio::test]
    async fn should_return_signature_on_success() {
        init_state();

        let expected_signature = signature();
        let runtime = TestCanisterRuntime::new().add_stub_response(
            SendTransactionResult::Consistent(Ok(expected_signature.clone())),
        );

        let result = submit_transaction(&runtime, transaction()).await;

        assert_eq!(result, Ok(expected_signature.into()));
    }

    #[tokio::test]
    async fn should_fail_on_ic_error() {
        init_state();

        let runtime = TestCanisterRuntime::new().add_stub_error(IcError::CallPerformFailed);

        let result = submit_transaction(&runtime, transaction()).await;

        assert_eq!(
            result,
            Err(SubmitTransactionError::IcError(IcError::CallPerformFailed))
        );
    }

    #[tokio::test]
    async fn should_fail_on_rpc_error() {
        init_state();

        let rpc_error = RpcError::HttpOutcallError(HttpOutcallError::InvalidHttpJsonRpcResponse {
            status: 500,
            body: "Internal server error".to_string(),
            parsing_error: None,
        });

        let runtime = TestCanisterRuntime::new()
            .add_stub_response(SendTransactionResult::Consistent(Err(rpc_error.clone())));

        let result = submit_transaction(&runtime, transaction()).await;

        assert_eq!(result, Err(SubmitTransactionError::RpcError(rpc_error)));
    }

    #[tokio::test]
    async fn should_fail_on_inconsistent_results() {
        init_state();

        let results = vec![
            (
                RpcSource::Supported(SupportedRpcProviderId::AnkrMainnet),
                Ok(solana_signature::Signature::from([0x11; 64]).into()),
            ),
            (
                RpcSource::Supported(SupportedRpcProviderId::DrpcMainnet),
                Ok(solana_signature::Signature::from([0x22; 64]).into()),
            ),
        ];

        let runtime = TestCanisterRuntime::new()
            .add_stub_response(SendTransactionResult::Inconsistent(results));

        let result = submit_transaction(&runtime, transaction()).await;

        assert_eq!(result, Err(SubmitTransactionError::InconsistentRpcResults));
    }

    fn transaction() -> Transaction {
        let message = Message::new(&[], None);
        Transaction {
            signatures: vec![signature().into()],
            message,
        }
    }

    fn signature() -> sol_rpc_types::Signature {
        solana_signature::Signature::from([0x42; 64]).into()
    }
}

mod get_recent_slot_and_blockhash_tests {
    use super::*;

    type GetSlotResult = sol_rpc_types::MultiRpcResult<sol_rpc_types::Slot>;
    type GetBlockResult = sol_rpc_types::MultiRpcResult<Option<sol_rpc_types::ConfirmedBlock>>;

    #[tokio::test]
    async fn should_return_blockhash_and_slot_on_success() {
        init_state();

        let slot = 978458723;
        let runtime = TestCanisterRuntime::new()
            .add_stub_response(GetSlotResult::Consistent(Ok(slot)))
            .add_stub_response(GetBlockResult::Consistent(Ok(Some(confirmed_block()))));

        let result = get_recent_slot_and_blockhash(&runtime).await;

        assert_eq!(result, Ok((slot, blockhash().into())));
    }

    #[tokio::test]
    async fn should_fail_after_retrying() {
        init_state();
        let runtime = TestCanisterRuntime::new()
            .add_stub_response(GetSlotResult::Consistent(Err(RpcError::ValidationError(
                "Error 1".to_string(),
            ))))
            .add_stub_response(GetSlotResult::Consistent(Err(RpcError::ValidationError(
                "Error 2".to_string(),
            ))))
            .add_stub_response(GetSlotResult::Consistent(Err(RpcError::ValidationError(
                "Error 3".to_string(),
            ))));

        let result = get_recent_slot_and_blockhash(&runtime).await;

        assert_matches!(result, Err(GetRecentBlockhashError::Failed(_)));
    }

    fn blockhash() -> sol_rpc_types::Hash {
        solana_hash::Hash::from([0x42; 32]).into()
    }
}

mod get_signatures_for_address_tests {
    use super::*;
    use sol_rpc_types::{
        ConfirmedTransactionStatusWithSignature, GetSignaturesForAddressParams, Pubkey,
    };

    type MultiRpcResult =
        sol_rpc_types::MultiRpcResult<Vec<ConfirmedTransactionStatusWithSignature>>;

    fn test_params() -> GetSignaturesForAddressParams {
        GetSignaturesForAddressParams {
            pubkey: "Cybe9JqZKtmhBoVGNHBxRVMUndZno5vNj5bS9GqTCty1"
                .parse::<Pubkey>()
                .unwrap(),
            commitment: None,
            min_context_slot: None,
            limit: None,
            before: None,
            until: None,
        }
    }

    fn confirmed_tx_status(signature_bytes: [u8; 64]) -> ConfirmedTransactionStatusWithSignature {
        ConfirmedTransactionStatusWithSignature {
            signature: solana_signature::Signature::from(signature_bytes).into(),
            slot: 12345,
            err: None,
            memo: None,
            block_time: None,
            confirmation_status: None,
        }
    }

    #[tokio::test]
    async fn should_fail_if_get_signatures_for_address_fails() {
        init_state();

        let runtime = TestCanisterRuntime::new().add_stub_error(IcError::CallPerformFailed);

        let result = get_signatures_for_address(&runtime, test_params()).await;

        assert_eq!(
            result,
            Err(GetSignaturesForAddressError::IcError(
                IcError::CallPerformFailed
            ))
        );
    }

    #[tokio::test]
    async fn should_fail_if_get_signatures_for_address_returns_rpc_error() {
        init_state();

        let rpc_error = RpcError::HttpOutcallError(HttpOutcallError::InvalidHttpJsonRpcResponse {
            status: 500,
            body: "Internal server error".to_string(),
            parsing_error: None,
        });

        let runtime = TestCanisterRuntime::new()
            .add_stub_response(MultiRpcResult::Consistent(Err(rpc_error.clone())));

        let result = get_signatures_for_address(&runtime, test_params()).await;

        assert_eq!(
            result,
            Err(GetSignaturesForAddressError::RpcError(rpc_error))
        );
    }

    #[tokio::test]
    async fn should_fail_if_get_signatures_for_address_result_inconsistent() {
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

        let runtime =
            TestCanisterRuntime::new().add_stub_response(MultiRpcResult::Inconsistent(results));

        let result = get_signatures_for_address(&runtime, test_params()).await;

        assert_eq!(
            result,
            Err(GetSignaturesForAddressError::InconsistentRpcResults)
        );
    }

    #[tokio::test]
    async fn should_return_empty_vec_if_no_signatures() {
        init_state();

        let runtime =
            TestCanisterRuntime::new().add_stub_response(MultiRpcResult::Consistent(Ok(vec![])));

        let result = get_signatures_for_address(&runtime, test_params()).await;

        assert_eq!(result, Ok(vec![]));
    }

    #[tokio::test]
    async fn should_return_signatures() {
        init_state();

        let expected = vec![
            confirmed_tx_status([0x11; 64]),
            confirmed_tx_status([0x22; 64]),
        ];

        let runtime = TestCanisterRuntime::new()
            .add_stub_response(MultiRpcResult::Consistent(Ok(expected.clone())));

        let result = get_signatures_for_address(&runtime, test_params()).await;

        assert_eq!(result, Ok(expected));
    }
}
