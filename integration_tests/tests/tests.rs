use assert_matches::assert_matches;
use candid::Principal;
use cksol_int_tests::{Setup, SetupBuilder};
use cksol_types::{DepositStatus, GetDepositAddressArgs, UpdateBalanceArgs};
use cksol_types::{MinterInfo, RetrieveSolArgs, RetrieveSolError, RetrieveSolStatus, Signature};
use cksol_types_internal::log::Priority;
use ic_pocket_canister_runtime::{JsonRpcRequestMatcher, JsonRpcResponse, MockHttpOutcallsBuilder};
use icrc_ledger_types::icrc1::account::{Account, Subaccount};
use serde_json::json;
use sol_rpc_types::Lamport;
use std::str::FromStr;

const SOME_SIGNATURE: &str =
    "4basP1hZDqgt1BYwh29mURz4zr8BcJgya2Y4AjmzXB5vtViLG6hZRxF9iypkxkfCJXhJTFW7jU1PyG8rHXvYd4Zp";

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

        const DEFAULT_CALLER_DEPOSIT_ADDRESS: &str = "3fnbpmbdVhcvLMAgyGirs64B4BFFftmmSpeq7tuDD6tY";

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
    async fn should_get_minter_info() {
        let setup = SetupBuilder::new().build().await;

        let minter_info = setup.minter().get_minter_info().await;

        assert_eq!(minter_info, MinterInfo { deposit_fee: 0 });

        setup.drop().await;
    }
}

mod retrieve_sol_tests {
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
        assert_matches!(err, RetrieveSolError::InsufficientFunds { balance: 0 });

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

    const DEPOSIT_AMOUNT: Lamport = 500_000_000;
    // The signature for an actual Solana Devnnet transaction depositing 0.1 SOL to `DEFAULT_CALLER_DEPOSIT_ADDRESS`
    const DEPOSIT_TRANSACTION_SIGNATURE: &str =
        "4basP1hZDqgt1BYwh29mURz4zr8BcJgya2Y4AjmzXB5vtViLG6hZRxF9iypkxkfCJXhJTFW7jU1PyG8rHXvYd4Zp";

    #[tokio::test]
    async fn should_update_balance_with_single_deposit() {
        let setup = SetupBuilder::new().build().await;

        let balance_before = setup
            .ledger()
            .balance_of(Account {
                owner: Setup::DEFAULT_CALLER,
                subaccount: None,
            })
            .await;
        assert_eq!(balance_before, 0);

        let mocks = MockHttpOutcallsBuilder::new()
            .given(get_deposit_transaction_request().with_id(0))
            .respond_with(get_deposit_transaction_response().with_id(0))
            .given(get_deposit_transaction_request().with_id(1))
            .respond_with(get_deposit_transaction_response().with_id(1))
            .given(get_deposit_transaction_request().with_id(2))
            .respond_with(get_deposit_transaction_response().with_id(2))
            .build();

        let deposit_signature = Signature::from_str(DEPOSIT_TRANSACTION_SIGNATURE).unwrap();

        let result = setup
            .minter()
            .with_http_mocks(mocks)
            .update_balance(UpdateBalanceArgs {
                owner: None,
                subaccount: None,
                signature: deposit_signature.clone(),
            })
            .await;
        assert_matches!(result, Ok(DepositStatus::Minted {
            minted_amount,
            signature,
            ..
        }) if minted_amount == DEPOSIT_AMOUNT - Setup::DEFAULT_DEPOSIT_FEE && signature == deposit_signature);

        let balance_after = setup
            .ledger()
            .balance_of(Account {
                owner: Setup::DEFAULT_CALLER,
                subaccount: None,
            })
            .await;
        assert_eq!(balance_after, DEPOSIT_AMOUNT - Setup::DEFAULT_DEPOSIT_FEE);

        setup.drop().await;
    }

    // Transaction obtained by executing the following with the Solana CLI:
    // $ solana transfer 3fnbpmbdVhcvLMAgyGirs64B4BFFftmmSpeq7tuDD6tY 0.5 --allow-unfunded-recipient
    fn get_deposit_transaction_request() -> JsonRpcRequestMatcher {
        JsonRpcRequestMatcher::with_method("getTransaction")
            .with_params(json!([
                DEPOSIT_TRANSACTION_SIGNATURE,
                {"encoding": "base64", "commitment": "finalized"}
            ]))
            .with_id(0)
    }

    // Response to `getTransaction` Solana RPC method obtained with:
    // $ curl --location 'https://api.devnet.solana.com' \
    //  --header 'Content-Type: application/json' \
    //  --data '{
    //      "jsonrpc": "2.0",
    //      "id": 1,
    //      "method": "getTransaction",
    //      "params": [
    //          "4basP1hZDqgt1BYwh29mURz4zr8BcJgya2Y4AjmzXB5vtViLG6hZRxF9iypkxkfCJXhJTFW7jU1PyG8rHXvYd4Zp",
    //          "base64"
    //      ]
    //  }'
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

            // `get_deposit_address` endpoint
            let result = minter
                .try_update_balance(UpdateBalanceArgs {
                    owner,
                    subaccount: None,
                    signature: some_signature(),
                })
                .await;
            assert_matches!(result, Err(s) => s.contains("the owner must be non-anonymous"));
        }

        setup.drop().await;
    }
}

fn some_signature() -> Signature {
    Signature::from_str(SOME_SIGNATURE).unwrap()
}
