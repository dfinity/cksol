use crate::Setup;
use async_trait::async_trait;
use cksol_types::{GetDepositAddressArgs, Signature, UpdateBalanceArgs};
use ic_pocket_canister_runtime::{
    ExecuteHttpOutcallMocks, JsonRpcRequestMatcher, JsonRpcResponse, MockHttpOutcalls,
};
use icrc_ledger_types::{
    icrc::generic_value::{ICRC3Value, Value},
    icrc1::account::Account,
};
use pocket_ic::nonblocking::PocketIc;
use serde_json::json;
use sol_rpc_types::Lamport;
use solana_address::{Address, address};
use std::{str::FromStr, sync::Arc};
use tokio::sync::Mutex;

pub const DEFAULT_CALLER_ACCOUNT: Account = Account {
    owner: Setup::DEFAULT_CALLER,
    subaccount: None,
};

pub const DEFAULT_CALLER_DEPOSIT_ADDRESS: &str = "Cybe9JqZKtmhBoVGNHBxRVMUndZno5vNj5bS9GqTCty1";
pub const MINTER_ADDRESS: Address = address!("5G64DcCfSFRTwZWSTjub1qGRYrJFLeNMkYjfgCfKi1fi");

pub const DEPOSIT_AMOUNT: Lamport = 500_000_000;
pub const EXPECTED_MINT_AMOUNT: Lamport = DEPOSIT_AMOUNT - Setup::DEFAULT_DEPOSIT_FEE;

/// Signature for a Solana transaction depositing [`DEPOSIT_AMOUNT`] lamports to
/// the address [`DEFAULT_CALLER_DEPOSIT_ADDRESS`].
/// Explorer link to transaction on Solana Devnet [here].
///
/// [here]: https://explorer.solana.com/tx/5N4jM4eZGdeKJdFVFM7pY5GU79juLiJE7gALPpYXD1fkZEWkwc2cMW48Frxo8HkbRxLiSy5WkqLSEwb48Mam4amT?cluster=devnet
pub const DEPOSIT_TRANSACTION_SIGNATURE: &str =
    "5N4jM4eZGdeKJdFVFM7pY5GU79juLiJE7gALPpYXD1fkZEWkwc2cMW48Frxo8HkbRxLiSy5WkqLSEwb48Mam4amT";

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
///         "5N4jM4eZGdeKJdFVFM7pY5GU79juLiJE7gALPpYXD1fkZEWkwc2cMW48Frxo8HkbRxLiSy5WkqLSEwb48Mam4amT",
///         "base64"
///     ]
/// }'
/// ```
pub fn get_deposit_transaction_response() -> JsonRpcResponse {
    JsonRpcResponse::from(json!({
        "jsonrpc": "2.0",
        "result": {
            "blockTime": 1772109375,
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
                    4895801440_u64,
                    500000000,
                    1
                ],
                "postTokenBalances": [],
                "preBalances": [
                    5395806440_u64,
                    0,
                    1
                ],
                "preTokenBalances": [],
                "rewards": [],
                "status": {
                    "Ok": null
                }
            },
            "slot": 444797867,
            "transaction": [
                "Ado7qZrS2+XlOxCKlqFvtqzPQwvkbexjBYX9skG0JPuuFkwMe84uuIJnkzJumblHEWfuckKgoFqAOtmU0e2/oA4BAAEDIg5JU11WGypQAKfOpxcE0+UIiKney1G6hf+6GRXcmsex8D/gzAX2xhtlU/yePL5FYisYvQgGX/u3TyCP76Ea9AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAANGG6Jzufiyr0XO6naCKA8ZwrP6mGXfGtQf97Ki/UleMBAgIAAQwCAAAAAGXNHQAAAAA=",
                "base64"
            ]
        },
        "id": 1
    }))
}

/// This wrapper around [`MockHttpOutcalls`] allows different instances of [`PocketIcRuntime`]
/// to share the same mocks. This is useful in tests where several requests are made concurrently,
/// but only one of them results in HTTP outcalls being executed.
///
/// [`PocketIcRuntime`]: ic_pocket_canister_runtime::PocketIcRuntime
#[derive(Clone)]
pub struct SharedMockHttpOutcalls(Arc<Mutex<MockHttpOutcalls>>);

impl SharedMockHttpOutcalls {
    pub fn new(mocks: MockHttpOutcalls) -> Self {
        Self(Arc::new(Mutex::new(mocks)))
    }
}

#[async_trait]
impl ExecuteHttpOutcallMocks for SharedMockHttpOutcalls {
    async fn execute_http_outcall_mocks(&mut self, runtime: &PocketIc) -> () {
        self.0
            .lock()
            .await
            .execute_http_outcall_mocks(runtime)
            .await
    }
}

pub fn get_memo(block: ICRC3Value) -> Vec<u8> {
    let block: Value = block.into();
    let block_map = block.as_map().expect("should be a map");
    let tx = block_map.get("tx").expect("should have a tx");
    let tx_map = tx.clone().as_map().expect("should be a map");
    let memo = tx_map.get("memo").expect("should have a memo");
    let memo_blob = memo.clone().as_blob().expect("memo should be a blob");
    memo_blob.into_vec()
}
