use crate::{
    constants::GET_RECENT_BLOCK_MAX_TRIES,
    rpc::{
        Block, BlockHeight, GetBalanceError, GetNonceAccountError, GetRecentBlockError,
        GetTransactionError, NonceAccount, SubmitTransactionError, get_balance, get_nonce_account,
        get_recent_block, get_transaction, submit_transaction,
    },
    test_fixtures::{
        MINTER_ADDRESS, confirmed_block, confirmed_block_at_height,
        deposit::{
            DEPOSIT_ADDRESS, legacy_deposit_transaction, legacy_deposit_transaction_signature,
        },
        durable_nonce, fetched, init_state, nonce_account_address, nonce_account_info,
        runtime::TestCanisterRuntime,
        uninitialized_nonce_account_info,
    },
};
use assert_matches::assert_matches;
use ic_canister_runtime::IcError;
use sol_rpc_types::{HttpOutcallError, RpcError, RpcSource, SupportedRpcProviderId};
use solana_transaction::{Message, Transaction};
use solana_transaction_status_client_types::{EncodedTransaction, TransactionBinaryEncoding};

mod get_balance_tests {
    use super::*;
    use sol_rpc_types::Lamport;

    type MultiRpcResult = sol_rpc_types::MultiRpcResult<Lamport>;

    #[tokio::test]
    async fn should_return_balance() {
        init_state();
        let runtime =
            TestCanisterRuntime::new().add_stub_response(MultiRpcResult::Consistent(Ok(42)));

        let result = get_balance(&runtime, DEPOSIT_ADDRESS).await;

        assert_eq!(result, Ok(42));
    }

    #[tokio::test]
    async fn should_fail_if_call_fails_or_results_are_wrong() {
        init_state();
        let rpc_error = RpcError::ValidationError("Error 1".to_string());
        let inconsistent = vec![(
            RpcSource::Supported(SupportedRpcProviderId::AnkrMainnet),
            Err(rpc_error.clone()),
        )];

        for (runtime, expected) in [
            (
                TestCanisterRuntime::new().add_stub_error(IcError::CallPerformFailed),
                GetBalanceError::IcError(IcError::CallPerformFailed),
            ),
            (
                TestCanisterRuntime::new()
                    .add_stub_response(MultiRpcResult::Consistent(Err(rpc_error.clone()))),
                GetBalanceError::RpcError(rpc_error.clone()),
            ),
            (
                TestCanisterRuntime::new()
                    .add_stub_response(MultiRpcResult::Inconsistent(inconsistent.clone())),
                GetBalanceError::InconsistentRpcResults,
            ),
        ] {
            let result = get_balance(&runtime, DEPOSIT_ADDRESS).await;

            assert_eq!(result, Err(expected));
        }
    }
}

// TODO DEFI-2643: Test behavior with cycles
mod get_transaction_tests {
    use super::*;

    type MultiRpcResult = sol_rpc_types::MultiRpcResult<
        Option<sol_rpc_types::EncodedConfirmedTransactionWithStatusMeta>,
    >;

    #[tokio::test]
    async fn should_fail_if_get_transaction_fails() {
        init_state();

        let runtime = TestCanisterRuntime::new().add_stub_error(IcError::CallPerformFailed);

        let result = get_transaction(&runtime, legacy_deposit_transaction_signature()).await;

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
            .add_stub_response(MultiRpcResult::Consistent(Err(rpc_error.clone())));

        let result = get_transaction(&runtime, legacy_deposit_transaction_signature()).await;

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

        let runtime =
            TestCanisterRuntime::new().add_stub_response(MultiRpcResult::Inconsistent(results));

        let result = get_transaction(&runtime, legacy_deposit_transaction_signature()).await;

        assert_eq!(result, Err(GetTransactionError::InconsistentRpcResults));
    }

    #[tokio::test]
    async fn should_return_empty_if_transaction_not_found() {
        init_state();

        let runtime =
            TestCanisterRuntime::new().add_stub_response(MultiRpcResult::Consistent(Ok(None)));

        let result = get_transaction(&runtime, legacy_deposit_transaction_signature()).await;

        assert_eq!(result, Ok(None))
    }

    #[tokio::test]
    async fn should_fail_if_returned_transaction_has_another_signature() {
        init_state();

        let runtime = TestCanisterRuntime::new().add_stub_response(MultiRpcResult::Consistent(Ok(
            Some(legacy_deposit_transaction().try_into().unwrap()),
        )));

        let queried = solana_signature::Signature::from([7; 64]);

        let result = get_transaction(&runtime, queried).await;

        assert_eq!(
            result,
            Err(GetTransactionError::SignatureMismatch {
                queried,
                returned: Some(Box::new(legacy_deposit_transaction_signature())),
            })
        );
    }

    #[tokio::test]
    async fn should_fail_if_returned_transaction_cannot_be_decoded() {
        init_state();

        let mut transaction = legacy_deposit_transaction();
        transaction.transaction.transaction = EncodedTransaction::Binary(
            "not a transaction".to_string(),
            TransactionBinaryEncoding::Base64,
        );

        let runtime = TestCanisterRuntime::new().add_stub_response(MultiRpcResult::Consistent(Ok(
            Some(transaction.try_into().unwrap()),
        )));

        let result = get_transaction(&runtime, legacy_deposit_transaction_signature()).await;

        assert_eq!(
            result,
            Err(GetTransactionError::UndecodableTransaction {
                queried: legacy_deposit_transaction_signature()
            })
        );
    }

    #[tokio::test]
    async fn should_fail_if_the_signature_does_not_sign_the_returned_message() {
        init_state();

        let mut transaction = legacy_deposit_transaction();
        let mut decoded = transaction
            .transaction
            .transaction
            .decode()
            .expect("BUG: the fixture transaction should decode");
        let solana_message::VersionedMessage::Legacy(message) = &mut decoded.message else {
            panic!("BUG: the fixture is a legacy transaction");
        };
        message.recent_blockhash = solana_hash::Hash::new_from_array([0x5A; 32]);
        transaction.transaction.transaction = EncodedTransaction::Binary(
            base64::Engine::encode(
                &base64::engine::general_purpose::STANDARD,
                bincode::serialize(&decoded).expect("BUG: the transaction should serialize"),
            ),
            TransactionBinaryEncoding::Base64,
        );

        let runtime = TestCanisterRuntime::new().add_stub_response(MultiRpcResult::Consistent(Ok(
            Some(transaction.try_into().unwrap()),
        )));

        let result = get_transaction(&runtime, legacy_deposit_transaction_signature()).await;

        assert_eq!(
            result,
            Err(GetTransactionError::InvalidSignature {
                queried: legacy_deposit_transaction_signature()
            })
        );
    }

    #[tokio::test]
    async fn should_return_transaction() {
        init_state();

        let runtime = TestCanisterRuntime::new().add_stub_response(MultiRpcResult::Consistent(Ok(
            Some(legacy_deposit_transaction().try_into().unwrap()),
        )));

        let result = get_transaction(&runtime, legacy_deposit_transaction_signature()).await;

        assert_eq!(result, Ok(Some(fetched(legacy_deposit_transaction()))))
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

mod get_nonce_account_tests {
    use super::*;
    use sol_rpc_types::{AccountData, AccountEncoding};

    type GetAccountInfoResult = sol_rpc_types::MultiRpcResult<Option<sol_rpc_types::AccountInfo>>;

    #[tokio::test]
    async fn should_return_the_authority_and_nonce_value() {
        init_state();

        let runtime = TestCanisterRuntime::new().add_stub_response(
            GetAccountInfoResult::Consistent(Ok(Some(nonce_account_info(MINTER_ADDRESS, 1)))),
        );

        let result = get_nonce_account(&runtime, nonce_account_address()).await;

        assert_eq!(
            result,
            Ok(NonceAccount {
                authority: MINTER_ADDRESS,
                nonce: durable_nonce(1),
            })
        );
    }

    #[tokio::test]
    async fn should_fail_if_account_not_found() {
        init_state();

        let runtime = TestCanisterRuntime::new()
            .add_stub_response(GetAccountInfoResult::Consistent(Ok(None)));

        let result = get_nonce_account(&runtime, nonce_account_address()).await;

        assert_eq!(result, Err(GetNonceAccountError::AccountNotFound));
    }

    #[tokio::test]
    async fn should_fail_if_account_is_not_a_non_executable_system_program_account() {
        init_state();

        let foreign_owner_account = sol_rpc_types::AccountInfo {
            owner: MINTER_ADDRESS.to_string(),
            ..nonce_account_info(MINTER_ADDRESS, 1)
        };
        let executable_account = sol_rpc_types::AccountInfo {
            executable: true,
            ..nonce_account_info(MINTER_ADDRESS, 1)
        };

        for account in [foreign_owner_account, executable_account] {
            let runtime = TestCanisterRuntime::new()
                .add_stub_response(GetAccountInfoResult::Consistent(Ok(Some(account))));

            let result = get_nonce_account(&runtime, nonce_account_address()).await;

            assert_matches!(
                result,
                Err(GetNonceAccountError::UnexpectedAccountMetadata { .. })
            );
        }
    }

    #[tokio::test]
    async fn should_fail_if_account_is_not_an_initialized_nonce_account() {
        init_state();

        let account_with_data = |data: &str| sol_rpc_types::AccountInfo {
            data: AccountData::Binary(data.to_string(), AccountEncoding::Base64),
            ..nonce_account_info(MINTER_ADDRESS, 1)
        };
        let invalid_base64_account = account_with_data("not base64!");
        let invalid_nonce_state_account = account_with_data("AAAA");

        for account in [
            uninitialized_nonce_account_info(),
            invalid_base64_account,
            invalid_nonce_state_account,
        ] {
            let runtime = TestCanisterRuntime::new()
                .add_stub_response(GetAccountInfoResult::Consistent(Ok(Some(account))));

            let result = get_nonce_account(&runtime, nonce_account_address()).await;

            assert_matches!(
                result,
                Err(GetNonceAccountError::NotAnInitializedNonceAccount(_))
            );
        }
    }
}

mod get_recent_block_tests {
    use super::*;

    type GetSlotResult = sol_rpc_types::MultiRpcResult<sol_rpc_types::Slot>;
    type GetBlockResult = sol_rpc_types::MultiRpcResult<Option<sol_rpc_types::ConfirmedBlock>>;

    const SLOT: sol_rpc_types::Slot = 978458723;

    #[tokio::test]
    async fn should_return_slot_blockhash_and_block_height_on_success() {
        init_state();
        let block_height = BlockHeight::new(SLOT - 10);
        let runtime = TestCanisterRuntime::new()
            .add_stub_response(GetSlotResult::Consistent(Ok(SLOT)))
            .add_stub_response(GetBlockResult::Consistent(Ok(Some(
                confirmed_block_at_height(block_height),
            ))));

        let result = get_recent_block(&runtime).await;

        assert_eq!(
            result,
            Ok(Block {
                slot: SLOT,
                blockhash: blockhash().into(),
                block_height,
            })
        );
    }

    #[tokio::test]
    async fn should_fail_when_block_has_no_block_height() {
        init_state();
        let runtime = TestCanisterRuntime::new()
            .add_stub_response(GetSlotResult::Consistent(Ok(SLOT)))
            .add_stub_response(GetBlockResult::Consistent(Ok(Some(
                sol_rpc_types::ConfirmedBlock {
                    block_height: None,
                    ..confirmed_block()
                },
            ))));

        let result = get_recent_block(&runtime).await;

        assert_eq!(
            result,
            Err(GetRecentBlockError::MissingBlockHeight { slot: SLOT })
        );
    }

    #[tokio::test]
    async fn should_fail_after_retrying() {
        init_state();
        let runtime = TestCanisterRuntime::new()
            .add_recent_block(Err(RpcError::ValidationError("Error".to_string())));

        let result = get_recent_block(&runtime).await;

        assert_matches!(
            result,
            Err(GetRecentBlockError::Failed(errors))
                if errors.len() == GET_RECENT_BLOCK_MAX_TRIES.get()
        );
    }

    fn blockhash() -> sol_rpc_types::Hash {
        solana_hash::Hash::from([0x42; 32]).into()
    }
}
