use assert_matches::assert_matches;
use candid::Principal;
use cksol_int_tests::{
    Setup, SetupBuilder,
    fixtures::{
        DEPOSIT_TRANSACTION_SIGNATURE, default_update_balance_args, deposit_transaction_signature,
    },
};
use cksol_types::{
    GetDepositAddressArgs, RetrieveSolArgs, RetrieveSolError, RetrieveSolStatus, UpdateBalanceArgs,
    UpdateBalanceError,
};
use ic_pocket_canister_runtime::{JsonRpcRequestMatcher, JsonRpcResponse, MockHttpOutcallsBuilder};
use icrc_ledger_types::icrc1::account::Subaccount;
use serde_json::json;

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

        const DEFAULT_CALLER_DEPOSIT_ADDRESS: &str = "Bp4HGmh5yPKB364FnKCfR8yVcMopLBTpJ8uCwqSKHH8H";

        // Owner is the default caller
        assert_eq!(
            get_deposit_address(&setup, None, None).await,
            DEFAULT_CALLER_DEPOSIT_ADDRESS
        );

        // Different owner
        assert_eq!(
            get_deposit_address(&setup, Some(Principal::from_slice(&[1])), None).await,
            "2fKa3spdjRZoCZ2hCzs5KEWM5RkVASCKdHpM2stuWgBV"
        );

        // Owner is the default caller, but different subaccounts specified
        assert_eq!(
            get_deposit_address(&setup, None, Some([1; 32])).await,
            "64MdjAMBn5YnL5iE5K5UifTqA4XjHW1xb7u7861wBjcf"
        );
        assert_eq!(
            get_deposit_address(&setup, None, Some([2; 32])).await,
            "244iFvwwqfX1PSvvGPBnsEnXV1MKEnNHfFTkUVsdtp1n"
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
    use cksol_int_tests::{Setup, SetupBuilder};
    use cksol_types::MinterInfo;
    use cksol_types_internal::UpgradeArgs;
    use cksol_types_internal::log::Priority;

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
                deposit_fee: 0,
                minimum_withdrawal_amount: Setup::DEFAULT_MINIMUM_WITHDRAWAL_AMOUNT
            }
        );

        let new_deposit_fee = 10;
        let new_minimum_withdrawal_amount = 20;
        setup
            .upgrade_minter(UpgradeArgs {
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
        assert_eq!(err, RetrieveSolError::InsufficientFunds { balance: 0 });

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
        assert_eq!(err, RetrieveSolError::InsufficientFunds { balance: 0 });

        let new_minimum_withdrawal_amount = Setup::DEFAULT_MINIMUM_WITHDRAWAL_AMOUNT + 1;
        setup
            .upgrade_minter(UpgradeArgs {
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
        assert_eq!(err, RetrieveSolError::InsufficientFunds { balance: 0 });

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
    async fn should_not_update_balance_if_transaction_not_found() {
        fn transaction_not_found_response() -> JsonRpcResponse {
            JsonRpcResponse::from(json!({"jsonrpc": "2.0", "result": null, "id": 0}))
        }

        let setup = SetupBuilder::new().build().await;

        let mocks = MockHttpOutcallsBuilder::new()
            .given(get_deposit_transaction_request().with_id(0))
            .respond_with(transaction_not_found_response().with_id(0))
            .given(get_deposit_transaction_request().with_id(1))
            .respond_with(transaction_not_found_response().with_id(1))
            .given(get_deposit_transaction_request().with_id(2))
            .respond_with(transaction_not_found_response().with_id(2))
            .given(get_deposit_transaction_request().with_id(3))
            .respond_with(transaction_not_found_response().with_id(3))
            .build();

        let result = setup
            .minter()
            .with_http_mocks(mocks)
            .update_balance(default_update_balance_args())
            .await;

        assert_eq!(result, Err(UpdateBalanceError::TransactionNotFound));

        setup.drop().await;
    }

    #[tokio::test]
    async fn should_update_balance() {
        let setup = SetupBuilder::new().build().await;

        let mocks = MockHttpOutcallsBuilder::new()
            .given(get_deposit_transaction_request().with_id(0))
            .respond_with(get_deposit_transaction_response().with_id(0))
            .given(get_deposit_transaction_request().with_id(1))
            .respond_with(get_deposit_transaction_response().with_id(1))
            .given(get_deposit_transaction_request().with_id(2))
            .respond_with(get_deposit_transaction_response().with_id(2))
            .given(get_deposit_transaction_request().with_id(3))
            .respond_with(get_deposit_transaction_response().with_id(3))
            .build();

        let result = setup
            .minter()
            .with_http_mocks(mocks)
            .update_balance(default_update_balance_args())
            .await;

        // TODO DEFI-2643: Change once deposit logic is implemented
        assert_matches!(result, Err(UpdateBalanceError::TemporarilyUnavailable(s)) => {
            assert!(s.contains("Not yet implemented!"))
        });

        setup.drop().await;
    }

    fn get_deposit_transaction_request() -> JsonRpcRequestMatcher {
        JsonRpcRequestMatcher::with_method("getTransaction")
            .with_params(json!([
                DEPOSIT_TRANSACTION_SIGNATURE,
                {"encoding": "base64", "commitment": "finalized"}
            ]))
            .with_id(0)
    }

    fn get_deposit_transaction_response() -> JsonRpcResponse {
        JsonRpcResponse::from(json!({
            "jsonrpc": "2.0",
            "result": {
                "blockTime": 1770997258,
                "meta": {
                    "computeUnitsConsumed": 150,
                    "costUnits": 1481,
                    "err": null,
                    "fee": 5000,
                    "innerInstructions": [],
                    "loadedAddresses": {
                        "readonly": [],
                        "writable": []
                    },
                    "logMessages": [
                        "Program 11111111111111111111111111111111 invoke [1]",
                        "Program 11111111111111111111111111111111 success"
                    ],
                    "postBalances": [
                        3395836440_u64,
                        500000000,
                        1
                    ],
                    "postTokenBalances": [],
                    "preBalances": [
                        3895841440_u64,
                        0,
                        1
                    ],
                    "preTokenBalances": [],
                    "rewards": [],
                    "status": {
                        "Ok": null
                    }
                },
                "slot": 441894876,
                "transaction": [
                    "AbPf97eQzgIgQGGFzEA2zvWWbaNdZxVOsN+Zem/HooxKiAzkImkLy/qXv56MOq0kQ9yJYWw4ZTOGP8mTemI6MgsBAAEDIg5JU11WGypQAKfOpxcE0+UIiKney1G6hf+6GRXcmscnpwFQ/UrMJ1PeTEdnddpynJZVZBAGM5/4YyiEZlx8QQAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA/ojmZ6HPuhM7YU56uETXnzzzvzHc55RxGfYTOIsoFu0BAgIAAQwCAAAAAGXNHQAAAAA=",
                    "base64"
                ]
            },
            "id": 1
        }))
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
