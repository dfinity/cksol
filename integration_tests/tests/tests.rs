use assert_matches::assert_matches;
use candid::Principal;
use cksol_int_tests::{Setup, SetupBuilder};
use cksol_types::{DepositStatus, GetDepositAddressArgs, UpdateBalanceArgs};
use ic_pocket_canister_runtime::{JsonRpcRequestMatcher, JsonRpcResponse, MockHttpOutcallsBuilder};
use icrc_ledger_types::icrc1::account::{Account, Subaccount};
use serde_json::json;
use sol_rpc_types::{Lamport, Signature};
use std::str::FromStr;

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

        const DEFAULT_CALLER_DEPOSIT_ADDRESS: &str = "Ge2aoiaTb6Tq2DQ4xs7qGhGud97pKtDmJCAQufTJeNSu";

        // Owner is the default caller
        assert_eq!(
            get_deposit_address(&setup, None, None).await,
            DEFAULT_CALLER_DEPOSIT_ADDRESS
        );

        // Different owner
        assert_eq!(
            get_deposit_address(&setup, Some(Principal::from_slice(&[1])), None).await,
            "9qvNPGSFQY8fvmr5A2jyCmSBfN7rrWBGJEAGgpN2TKeV"
        );

        // Owner is the default caller, but different subaccounts specified
        assert_eq!(
            get_deposit_address(&setup, None, Some([1; 32])).await,
            "97eLNQ1sc7yQHscLWet7vq7AZ6TbxN5nx8D8LPSbYEJB"
        );
        assert_eq!(
            get_deposit_address(&setup, None, Some([2; 32])).await,
            "BiuUj1yMbtStuumWutpBajSjNDPbnE5dNEuTv7J1cjmB"
        );

        // Caller is anonymous, but we specify the owner explicitly
        let setup = SetupBuilder::new()
            .with_caller(Principal::anonymous())
            .build()
            .await;

        assert_eq!(
            get_deposit_address(&setup, Some(Setup::DEFAULT_CALLER), None).await,
            DEFAULT_CALLER_DEPOSIT_ADDRESS
        );
    }

    #[tokio::test]
    async fn should_fail_for_anonymous_owner() {
        let setup = SetupBuilder::new().build().await;

        // Caller is default caller, but the owner is specified explicitly to anonymous
        let result = setup
            .minter()
            .try_get_deposit_address(GetDepositAddressArgs {
                owner: Some(Principal::anonymous()),
                subaccount: None,
            })
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("the owner must be non-anonymous"));

        // Anonymous caller and owner not specified
        let setup = SetupBuilder::new()
            .with_caller(Principal::anonymous())
            .build()
            .await;

        let result = setup
            .minter()
            .try_get_deposit_address(GetDepositAddressArgs {
                owner: None,
                subaccount: None,
            })
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("the owner must be non-anonymous"));
    }
}

mod update_balance_tests {
    use super::*;

    const DEPOSIT_AMOUNT: Lamport = 500_000_000;
    // The signature for an actual Solana Devnnet transaction depositing 0.1 SOL to `DEPOSIT_ADDRESS`
    const DEPOSIT_TRANSACTION_SIGNATURE: &str =
        "4VZbtG9SyqkuJ4UFG4VJcMNQZ7Vvpsp5S8qe5Z9gQEasm2WMNKEaWvwuJZ5nUybTsNEiXUBYaxwekgiPHpdmFLKU";

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
    }

    // Transaction obtained by executing the following with the Solana CLI:
    // $ solana transfer 5pGhcgcYFiH1XkX8sSDb73d3GUGLiqb1xFiPHe1Us5gd 0.5 --allow-unfunded-recipient
    fn get_deposit_transaction_request() -> JsonRpcRequestMatcher {
        JsonRpcRequestMatcher::with_method("getTransaction")
            .with_params(json!([DEPOSIT_TRANSACTION_SIGNATURE, {"encoding": "base64"}]))
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
    //          "3k59e3o79fFv86ZvQJat1MpbTUmRbocWfXwgx8139yyaenVGDPqoyUaJ3ieiRaUdWQ7pjpAUDqaohw9NyZkBjzkq",
    //          "base64"
    //      ]
    //  }'
    fn get_deposit_transaction_response() -> JsonRpcResponse {
        JsonRpcResponse::from(json!({
            "jsonrpc": "2.0",
            "result": {
                "blockTime": 1770823711,
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
                        4895851440_u64,
                        500000000,
                        1
                    ],
                    "postTokenBalances": [],
                    "preBalances": [
                        5395856440_u64,
                        0,
                        1
                    ],
                    "preTokenBalances": [],
                    "rewards": [],
                    "status": {
                        "Ok": null
                    }
                },
                "slot": 441440019,
                "transaction": [
                    "AYktYGiT43PlVKQj4GkvJNEYsFf46SjZFlIIJfrwsC5+F9RSD7CmCEq9VZEUQhb8mstaW1kZq9YJC0qclWf+NwIBAAEDIg5JU11WGypQAKfOpxcE0+UIiKney1G6hf+6GRXcmsdHisxOqffyzXZdIsfZqk+EH/pXAFqikE3sCFw3rfo5SgAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAtkASUDT5PLxRnNxUcWC1DKt8jbIVXIzaEWXKr6vu+IBAgIAAQwCAAAAAGXNHQAAAAA=",
                    "base64"
                ]
            },
            "id": 1
        }))
    }
}
