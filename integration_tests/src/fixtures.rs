use crate::Setup;
use cksol_types::{GetDepositAddressArgs, Signature, UpdateBalanceArgs};
use ic_pocket_canister_runtime::{JsonRpcRequestMatcher, JsonRpcResponse};
use icrc_ledger_types::icrc1::account::Account;
use serde_json::json;
use sol_rpc_types::Lamport;
use std::str::FromStr;

pub const DEFAULT_CALLER_ACCOUNT: Account = Account {
    owner: Setup::DEFAULT_CALLER,
    subaccount: None,
};

pub const DEFAULT_CALLER_DEPOSIT_ADDRESS: &str = "3fnbpmbdVhcvLMAgyGirs64B4BFFftmmSpeq7tuDD6tY";

pub const DEPOSIT_AMOUNT: Lamport = 500_000_000;

/// Signature for a Solana transaction depositing [`DEPOSIT_AMOUNT`] lamports to
/// the address [`DEFAULT_CALLER_DEPOSIT_ADDRESS`].
/// Explorer link to transaction on Solana Devnet [here].
///
/// [here]: https://explorer.solana.com/tx/5b5QLKzj24LtvBLSyKkQCrdSDp9Y66y48ns2vxbp4qTHnRSYd1jtFW9vwKXjbyLFFNpNupcRdvhsCpHTc7g6E77U?cluster=devnet
pub const DEPOSIT_TRANSACTION_SIGNATURE: &str =
    "5b5QLKzj24LtvBLSyKkQCrdSDp9Y66y48ns2vxbp4qTHnRSYd1jtFW9vwKXjbyLFFNpNupcRdvhsCpHTc7g6E77U";

pub fn deposit_transaction_signature() -> Signature {
    Signature::from_str(DEPOSIT_TRANSACTION_SIGNATURE).unwrap()
}

pub fn default_get_deposit_address_args() -> GetDepositAddressArgs {
    GetDepositAddressArgs {
        owner: None,
        subaccount: None,
    }
}

pub fn default_update_balance_args() -> UpdateBalanceArgs {
    UpdateBalanceArgs {
        owner: None,
        subaccount: None,
        signature: deposit_transaction_signature(),
    }
}

/// [`getTransaction`] request for [`DEPOSIT_TRANSACTION_SIGNATURE`].
pub fn get_deposit_transaction_request() -> JsonRpcRequestMatcher {
    JsonRpcRequestMatcher::with_method("getTransaction").with_params(json!([
        DEPOSIT_TRANSACTION_SIGNATURE,
        {"encoding": "base64", "commitment": "finalized"}
    ]))
}

/// JSON-RPC response for [`get_deposit_transaction_request`].
/// Can be obtained with the following `curl` command:
/// ```bash
/// curl --location 'https://api.devnet.solana.com' \
/// --header 'Content-Type: application/json' \
/// --data '{
///     "jsonrpc": "2.0",
///     "id": 1,
///     "method": "getTransaction",
///     "params": [
///         "5b5QLKzj24LtvBLSyKkQCrdSDp9Y66y48ns2vxbp4qTHnRSYd1jtFW9vwKXjbyLFFNpNupcRdvhsCpHTc7g6E77U",
///         "base64"
///     ]
/// }'
/// ```
pub fn get_deposit_transaction_response() -> JsonRpcResponse {
    JsonRpcResponse::from(json!({
        "jsonrpc": "2.0",
        "result": {
            "blockTime": 1771842069,
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
                    395806440,
                    1000000000,
                    1
                ],
                "postTokenBalances": [],
                "preBalances": [
                    895811440,
                    500000000,
                    1
                ],
                "preTokenBalances": [],
                "rewards": [],
                "status": {
                    "Ok": null
                }
            },
            "slot": 444101463,
            "transaction": [
                "AeV0KXYwhK0c6hKAXSKU0imPXE6vdSbzek8yUgxLbdGelH5CfCBX/r0R973eRJm/cece7VCf63bfHPXC8px69AcBAAEDIg5JU11WGypQAKfOpxcE0+UIiKney1G6hf+6GRXcmscnpwFQ/UrMJ1PeTEdnddpynJZVZBAGM5/4YyiEZlx8QQAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAOi6jUt2z2U/z9Kr4J2FrD7kS9YN/76NVpbnBD27jOzQBAgIAAQwCAAAAAGXNHQAAAAA=",
                "base64"
            ]
        },
        "id": 1
    }))
}
