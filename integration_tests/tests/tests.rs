use assert_matches::assert_matches;
use assert2::check;
use candid::Principal;
use cksol_int_tests::{
    Setup, SetupBuilder,
    fixtures::{
        DEFAULT_CALLER_ACCOUNT, DEFAULT_CALLER_DEPOSIT_ADDRESS, DEPOSIT_AMOUNT,
        SharedMockHttpOutcalls, default_update_balance_args, deposit_transaction_signature,
        get_deposit_transaction_request, get_deposit_transaction_response,
    },
};
use cksol_types::{
    DepositStatus, GetDepositAddressArgs, MinterInfo, RetrieveSolArgs, RetrieveSolError,
    RetrieveSolStatus, UpdateBalanceArgs, UpdateBalanceError,
};
use cksol_types_internal::{UpgradeArgs, event::EventType, log::Priority};
use ic_pocket_canister_runtime::{JsonRpcResponse, MockHttpOutcalls, MockHttpOutcallsBuilder};
use icrc_ledger_types::icrc1::account::Subaccount;
use serde_json::json;
use tokio::join;

mod get_deposit_address_tests {
    use super::*;

    async fn get_deposit_address(
        setup: &Setup,
        owner: Option<Principal>,
        subaccount: Option<Subaccount>,
    ) -> String {
        setup
            .minter()
            .get_deposit_address(GetDepositAddressArgs { owner, subaccount })
            .await
            .to_string()
    }

    #[tokio::test]
    async fn should_get_deposit_address() {
        let setup = SetupBuilder::new().build().await;

        // Owner is the default caller
        assert_eq!(
            get_deposit_address(&setup, None, None).await,
            DEFAULT_CALLER_DEPOSIT_ADDRESS
        );

        // Different owner
        assert_eq!(
            get_deposit_address(&setup, Some(Principal::from_slice(&[1])), None).await,
            "Dyh5A77LtkkYan5NJH4vvCji7WJKBQEqCDupPtmUpxoE"
        );

        // Owner is the default caller, but different subaccounts specified
        assert_eq!(
            get_deposit_address(&setup, None, Some([1; 32])).await,
            "HB8XFVocoLig1KKpp5w41noDi4QN7SUx6HPWV7CKsaVR"
        );
        assert_eq!(
            get_deposit_address(&setup, None, Some([2; 32])).await,
            "Hu9cz6aPzLcyJWexefTthALmKBKZTiqt5TomTg2qwD2N"
        );

        setup.drop().await;

        // Caller is anonymous, but we specify the owner explicitly
        let setup = SetupBuilder::new()
            .with_caller(Principal::anonymous())
            .build()
            .await;

        assert_eq!(
            get_deposit_address(&setup, Some(Setup::DEFAULT_CALLER), None).await,
            DEFAULT_CALLER_DEPOSIT_ADDRESS
        );

        setup.drop().await;
    }
}

mod lifecycle {
    use super::*;

    #[tokio::test]
    async fn should_get_logs() {
        let setup = SetupBuilder::new().build().await;

        let logs = setup.minter().retrieve_logs(&Priority::Info).await;

        assert!(logs[0].message.contains("[init]"));

        setup.drop().await;
    }

    #[tokio::test]
    async fn should_get_minter_info_and_upgrade() {
        let setup = SetupBuilder::new().build().await;

        let minter_info = setup.minter().get_minter_info().await;
        assert_eq!(
            minter_info,
            MinterInfo {
                deposit_fee: Setup::DEFAULT_DEPOSIT_FEE,
                minimum_withdrawal_amount: Setup::DEFAULT_MINIMUM_WITHDRAWAL_AMOUNT,
            }
        );

        let new_deposit_fee = 10;
        let new_minimum_withdrawal_amount = 20;
        setup
            .minter()
            .upgrade(UpgradeArgs {
                deposit_fee: Some(new_deposit_fee),
                minimum_withdrawal_amount: Some(new_minimum_withdrawal_amount),
                ..Default::default()
            })
            .await
            .expect("upgrade failed");

        let minter_info = setup.minter().get_minter_info().await;
        assert_eq!(
            minter_info,
            MinterInfo {
                deposit_fee: new_deposit_fee,
                minimum_withdrawal_amount: new_minimum_withdrawal_amount,
            }
        );

        setup.drop().await;
    }

    #[tokio::test]
    async fn should_get_events() {
        let setup = SetupBuilder::new().build().await;
        let minter = setup.minter();

        minter.assert_that_events().await.satisfy(|events| {
            check!(events.len() == 1 && matches!(events[0], EventType::Init(_)));
        });

        minter
            .upgrade(Default::default())
            .await
            .expect("upgrade failed");

        minter.assert_that_events().await.satisfy(|events| {
            check!(events.len() == 2 && matches!(events[1], EventType::Upgrade(_)));
        });

        setup.drop().await;
    }
}

mod retrieve_sol_tests {
    use cksol_types_internal::UpgradeArgs;

    use super::*;

    #[tokio::test]
    async fn should_validate_solana_address() {
        let setup = SetupBuilder::new().build().await;

        let args = RetrieveSolArgs {
            from_subaccount: None,
            amount: u64::MAX,
            address: "InvalidAddress".to_string(),
        };

        let result = setup.minter().retrieve_sol(args).await;
        let err = result.unwrap_err();
        assert_matches!(err, RetrieveSolError::MalformedAddress(_));

        let args = RetrieveSolArgs {
            from_subaccount: None,
            amount: u64::MAX,
            address: "E4MpwNnMWs2XtW5gVrxZvyS7fMq31QD5HvbxmwP45Tz3".to_string(),
        };

        let result = setup.minter().retrieve_sol(args).await;
        let err = result.unwrap_err();
        assert_eq!(err, RetrieveSolError::InsufficientAllowance { allowance: 0 });

        setup.drop().await;
    }

    #[tokio::test]
    async fn should_check_minimum_withdrawal_amount() {
        let setup = SetupBuilder::new().build().await;

        let args = RetrieveSolArgs {
            from_subaccount: None,
            amount: Setup::DEFAULT_MINIMUM_WITHDRAWAL_AMOUNT,
            address: "E4MpwNnMWs2XtW5gVrxZvyS7fMq31QD5HvbxmwP45Tz3".to_string(),
        };

        let result = setup.minter().retrieve_sol(args.clone()).await;
        let err = result.unwrap_err();
        assert_eq!(err, RetrieveSolError::InsufficientAllowance { allowance: 0 });

        let new_minimum_withdrawal_amount = Setup::DEFAULT_MINIMUM_WITHDRAWAL_AMOUNT + 1;
        setup
            .minter()
            .upgrade(UpgradeArgs {
                minimum_withdrawal_amount: Some(new_minimum_withdrawal_amount),
                ..Default::default()
            })
            .await
            .expect("upgrade failed");

        let result = setup.minter().retrieve_sol(args).await;
        let err = result.unwrap_err();
        assert_eq!(
            err,
            RetrieveSolError::AmountTooLow(new_minimum_withdrawal_amount)
        );

        let args = RetrieveSolArgs {
            from_subaccount: None,
            amount: new_minimum_withdrawal_amount,
            address: "E4MpwNnMWs2XtW5gVrxZvyS7fMq31QD5HvbxmwP45Tz3".to_string(),
        };

        let result = setup.minter().retrieve_sol(args).await;
        let err = result.unwrap_err();
        assert_eq!(err, RetrieveSolError::InsufficientAllowance { allowance: 0 });

        setup.drop().await;
    }

    #[tokio::test]
    async fn should_return_not_found_status() {
        let setup = SetupBuilder::new().build().await;

        let status = setup.minter().retrieve_sol_status(u64::MAX).await;
        assert_eq!(status, RetrieveSolStatus::NotFound);

        setup.drop().await;
    }
}

mod update_balance_tests {
    use super::*;

    #[tokio::test]
    async fn should_fail_if_transaction_not_found() {
        fn transaction_not_found_response() -> JsonRpcResponse {
            JsonRpcResponse::from(json!({"jsonrpc": "2.0", "result": null, "id": 0}))
        }

        let setup = SetupBuilder::new().build().await;

        let result = setup
            .minter()
            .with_http_mocks(get_transaction_http_mocks(transaction_not_found_response))
            .update_balance(default_update_balance_args())
            .await;

        assert_eq!(result, Err(UpdateBalanceError::TransactionNotFound));

        setup.drop().await;
    }

    #[tokio::test]
    async fn should_fail_for_concurrent_access() {
        let setup = SetupBuilder::new().build().await;

        // Both minters use the same mocks, whichever gets the guard first will consume them
        let mocks = SharedMockHttpOutcalls::new(get_transaction_http_mocks(
            get_deposit_transaction_response,
        ));

        let minter1 = setup.minter().with_http_mocks(mocks.clone());
        let minter2 = setup.minter().with_http_mocks(mocks.clone());

        let (result1, result2) = join!(
            minter1.update_balance(default_update_balance_args()),
            minter2.update_balance(default_update_balance_args())
        );

        let (result1, result2) = match (&result1, &result2) {
            (Ok(_), Err(_)) => (result1, result2),
            (Err(_), Ok(_)) => (result2, result1),
            _ => panic!("Expected one success and one error, but got: {result1:?} and {result2:?}"),
        };

        // One should succeed, one should fail with AlreadyProcessing (order is non-deterministic)
        let results = [&result1, &result2];
        assert!(
            results
                .iter()
                .any(|r| matches!(r, Ok(DepositStatus::Minted { .. }))),
            "Expected one Minted result, got: {:?}",
            results
        );
        assert!(
            results
                .iter()
                .any(|r| matches!(r, Err(UpdateBalanceError::AlreadyProcessing))),
            "Expected one AlreadyProcessing result, got: {:?}",
            results
        );

        setup.drop().await;
    }

    #[tokio::test]
    async fn should_return_processing_if_minting_fails() {
        let setup = SetupBuilder::new().build().await;

        setup.ledger().stop().await;

        let deposit_signature = deposit_transaction_signature();

        let result = setup
            .minter()
            .with_http_mocks(get_transaction_http_mocks(get_deposit_transaction_response))
            .update_balance(default_update_balance_args())
            .await;
        assert_eq!(result, Ok(DepositStatus::Processing(deposit_signature)));

        setup.drop().await;
    }

    #[tokio::test]
    async fn should_update_balance_with_single_deposit() {
        let setup = SetupBuilder::new().build().await;

        let balance_before = setup.ledger().balance_of(DEFAULT_CALLER_ACCOUNT).await;
        assert_eq!(balance_before, 0);

        let deposit_signature = deposit_transaction_signature();

        let result = setup
            .minter()
            .with_http_mocks(get_transaction_http_mocks(get_deposit_transaction_response))
            .update_balance(default_update_balance_args())
            .await;
        let expected_minted_amount = DEPOSIT_AMOUNT - Setup::DEFAULT_DEPOSIT_FEE;
        assert_matches!(result, Ok(DepositStatus::Minted {
            minted_amount,
            signature,
            block_index: _,
        }) if minted_amount == expected_minted_amount && signature == deposit_signature);

        let balance_after = setup.ledger().balance_of(DEFAULT_CALLER_ACCOUNT).await;
        assert_eq!(balance_after, expected_minted_amount);

        setup.drop().await;
    }

    fn get_transaction_http_mocks(response: impl Fn() -> JsonRpcResponse) -> MockHttpOutcalls {
        MockHttpOutcallsBuilder::new()
            .given(get_deposit_transaction_request().with_id(0))
            .respond_with(response().with_id(0))
            .given(get_deposit_transaction_request().with_id(1))
            .respond_with(response().with_id(1))
            .given(get_deposit_transaction_request().with_id(2))
            .respond_with(response().with_id(2))
            .given(get_deposit_transaction_request().with_id(3))
            .respond_with(response().with_id(3))
            .build()
    }
}

mod anonymous_caller_tests {
    use super::*;

    #[tokio::test]
    async fn should_fail_for_anonymous_owner() {
        let mut setup = SetupBuilder::new().build().await;

        for (caller, owner) in [
            // Caller is default caller, but the owner is specified explicitly to anonymous
            (Setup::DEFAULT_CALLER, Some(Principal::anonymous())),
            // Anonymous caller and owner not specified
            (Principal::anonymous(), None),
        ] {
            setup = setup.with_caller(caller);
            let minter = setup.minter();

            // `get_deposit_address` endpoint
            let result = minter
                .try_get_deposit_address(GetDepositAddressArgs {
                    owner,
                    subaccount: None,
                })
                .await;
            assert_matches!(result, Err(s) => s.contains("the owner must be non-anonymous"));

            // `update_balance` endpoint
            let result = minter
                .try_update_balance(UpdateBalanceArgs {
                    owner,
                    subaccount: None,
                    signature: deposit_transaction_signature(),
                })
                .await;
            assert_matches!(result, Err(s) => s.contains("the owner must be non-anonymous"));
        }

        setup.drop().await;
    }
}
