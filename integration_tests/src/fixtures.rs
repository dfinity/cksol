use crate::Setup;
use async_trait::async_trait;
use cksol_types::{GetDepositAddressArgs, ProcessDepositArgs, Signature};
use ic_pocket_canister_runtime::{
    ExecuteHttpOutcallMocks, JsonRpcRequestMatcher, JsonRpcResponse, MockHttpOutcalls,
    MockHttpOutcallsBuilder,
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
pub const EXPECTED_MINT_AMOUNT: Lamport = DEPOSIT_AMOUNT - Setup::DEFAULT_MANUAL_DEPOSIT_FEE;

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

pub fn default_update_balance_args() -> cksol_types::UpdateBalanceArgs {
    cksol_types::UpdateBalanceArgs {
        owner: None,
        subaccount: None,
    }
}

pub fn default_process_deposit_args() -> ProcessDepositArgs {
    ProcessDepositArgs {
        owner: None,
        subaccount: None,
        signature: deposit_transaction_signature(),
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

/// Thin wrapper around [`MockHttpOutcallsBuilder`] that auto-increments JSON-RPC IDs
/// in steps of [`NUM_RPC_PROVIDERS`] (one ID per redundant RPC provider).
pub struct MockBuilder {
    inner: MockHttpOutcallsBuilder,
    next_id: u64,
}

/// Number of Solana RPC providers used for redundancy.
/// Each logical RPC call generates this many HTTP outcalls with consecutive IDs.
pub const NUM_RPC_PROVIDERS: u64 = 4;

impl Default for MockBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl MockBuilder {
    pub fn new() -> Self {
        Self {
            inner: MockHttpOutcallsBuilder::new(),
            next_id: 0,
        }
    }

    pub fn with_start_id(id: u64) -> Self {
        Self {
            inner: MockHttpOutcallsBuilder::new(),
            next_id: id,
        }
    }

    /// Add a mock for one RPC call ([`NUM_RPC_PROVIDERS`] IDs for redundancy).
    pub fn expect(mut self, request: JsonRpcRequestMatcher, response: JsonRpcResponse) -> Self {
        for id in self.next_id..self.next_id + NUM_RPC_PROVIDERS {
            self.inner = self
                .inner
                .given(request.clone().with_id(id))
                .respond_with(response.clone().with_id(id));
        }
        self.next_id += NUM_RPC_PROVIDERS;
        self
    }

    pub fn build(self) -> MockHttpOutcalls {
        self.inner.build()
    }

    /// Mock for `getTransaction` with the given response.
    pub fn get_transaction(self, response: JsonRpcResponse) -> Self {
        self.expect(get_deposit_transaction_request(), response)
    }

    /// Mock for `getTransaction` returning the default deposit transaction.
    pub fn get_deposit_transaction(self) -> Self {
        self.get_transaction(get_deposit_transaction_response())
    }

    /// Mocks for `getSlot` → `getBlock`.
    pub fn get_slot_and_block(self, slot: u64, blockhash: &str) -> Self {
        self.expect(get_slot_request(), get_slot_response(slot))
            .expect(get_block_request(slot), get_block_response(blockhash))
    }

    /// Mocks for `getSlot` → `getBlock` → `sendTransaction`.
    pub fn submit_transaction(self, slot: u64, blockhash: &str, tx_signature: &str) -> Self {
        self.expect(get_slot_request(), get_slot_response(slot))
            .expect(get_block_request(slot), get_block_response(blockhash))
            .expect(
                send_transaction_request(),
                send_transaction_response(tx_signature),
            )
    }

    /// Mock for `getSignatureStatuses` returning not-found for `count` signatures.
    pub fn check_signature_statuses_not_found(self, count: usize) -> Self {
        self.expect(
            get_signature_statuses_request(),
            get_signature_statuses_not_found_response(count),
        )
    }

    /// Mock for `getSignatureStatuses` returning finalized for `count` signatures.
    pub fn check_signature_statuses_finalized(self, count: usize) -> Self {
        self.expect(
            get_signature_statuses_request(),
            get_signature_statuses_finalized_response(count),
        )
    }

    /// Mocks for `getSlot` → `getBlock`, used by the monitor timer to snapshot the current slot.
    pub fn get_current_slot(self, slot: u64, blockhash: &str) -> Self {
        self.expect(get_slot_request(), get_slot_response(slot))
            .expect(get_block_request(slot), get_block_response(blockhash))
    }

    /// Mocks for resubmitting an expired transaction:
    /// `getSlot` → `getBlock` → `getSignatureStatuses`(not found) → `getSlot` → `getBlock` → `sendTransaction`.
    pub fn resubmit_transaction(self, slot: u64, blockhash: &str, tx_signature: &str) -> Self {
        self.expect(get_slot_request(), get_slot_response(slot))
            .expect(get_block_request(slot), get_block_response(blockhash))
            .check_signature_statuses_not_found(1)
            .expect(get_slot_request(), get_slot_response(slot))
            .expect(get_block_request(slot), get_block_response(blockhash))
            .expect(
                send_transaction_request(),
                send_transaction_response(tx_signature),
            )
    }

    /// Mock for `getSignaturesForAddress` returning the given list of signature objects.
    pub fn get_signatures_for_address(self, signatures: Vec<serde_json::Value>) -> Self {
        self.expect(
            get_signatures_for_address_request(),
            get_signatures_for_address_response(signatures),
        )
    }

    /// Mock for `getBalance` returning the given lamport balance.
    pub fn get_balance(self, balance: Lamport) -> Self {
        self.expect(get_balance_request(), get_balance_response(balance))
    }
}

// ── JSON-RPC request matchers and response builders ─────────────────────────
// These are private helpers used by `MockBuilder` methods above.

/// [`getTransaction`] request for [`DEPOSIT_TRANSACTION_SIGNATURE`].
fn get_deposit_transaction_request() -> JsonRpcRequestMatcher {
    JsonRpcRequestMatcher::with_method("getTransaction").with_params(json!([
        DEPOSIT_TRANSACTION_SIGNATURE,
        {"encoding": "base64", "commitment": "finalized", "maxSupportedTransactionVersion": 0}
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
fn get_deposit_transaction_response() -> JsonRpcResponse {
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

fn get_slot_request() -> JsonRpcRequestMatcher {
    JsonRpcRequestMatcher::with_method("getSlot")
}

fn get_slot_response(slot: u64) -> JsonRpcResponse {
    JsonRpcResponse::from(json!({
        "jsonrpc": "2.0",
        "result": slot,
        "id": 1
    }))
}

fn get_block_request(slot: u64) -> JsonRpcRequestMatcher {
    // The SOL RPC canister rounds the slot down to the nearest multiple of 20
    // before making getBlock requests, so we match that behavior here.
    let slot = slot / 20 * 20;
    JsonRpcRequestMatcher::with_method("getBlock").with_params(json!([
        slot,
        {
            "transactionDetails": "none",
            "rewards": false,
            "maxSupportedTransactionVersion": 0
        }
    ]))
}

fn get_block_response(blockhash: &str) -> JsonRpcResponse {
    JsonRpcResponse::from(json!({
        "jsonrpc": "2.0",
        "result": {
            "blockhash": blockhash,
            "previousBlockhash": "CzBVNFJkh7WkQDfJUiDjLc7kPrJd8kR2yiCvwBUhSe7Y",
            "parentSlot": 449819444,
            "blockTime": 1700000000_i64,
            "blockHeight": 449819444
        },
        "id": 1
    }))
}

fn get_signature_statuses_request() -> JsonRpcRequestMatcher {
    JsonRpcRequestMatcher::with_method("getSignatureStatuses")
}

fn get_signature_statuses_not_found_response(count: usize) -> JsonRpcResponse {
    JsonRpcResponse::from(json!({
        "jsonrpc": "2.0",
        "result": {
            "context": { "slot": 0 },
            "value": vec![serde_json::Value::Null; count]
        },
        "id": 1
    }))
}

fn get_signature_statuses_finalized_response(count: usize) -> JsonRpcResponse {
    let statuses: Vec<_> = (0..count)
        .map(|_| {
            json!({
                "slot": 350_000_000_u64,
                "confirmations": null,
                "status": { "Ok": null },
                "err": null,
                "confirmationStatus": "finalized"
            })
        })
        .collect();
    JsonRpcResponse::from(json!({
        "jsonrpc": "2.0",
        "result": {
            "context": { "slot": 0 },
            "value": statuses
        },
        "id": 1
    }))
}

fn send_transaction_request() -> JsonRpcRequestMatcher {
    JsonRpcRequestMatcher::with_method("sendTransaction")
}

fn send_transaction_response(signature: &str) -> JsonRpcResponse {
    JsonRpcResponse::from(json!({
        "jsonrpc": "2.0",
        "result": signature,
        "id": 1
    }))
}

fn get_signatures_for_address_request() -> JsonRpcRequestMatcher {
    JsonRpcRequestMatcher::with_method("getSignaturesForAddress")
}

fn get_signatures_for_address_response(signatures: Vec<serde_json::Value>) -> JsonRpcResponse {
    JsonRpcResponse::from(json!({
        "jsonrpc": "2.0",
        "result": signatures,
        "id": 1
    }))
}

fn get_balance_request() -> JsonRpcRequestMatcher {
    JsonRpcRequestMatcher::with_method("getBalance")
}

fn get_balance_response(balance: Lamport) -> JsonRpcResponse {
    JsonRpcResponse::from(json!({
        "jsonrpc": "2.0",
        "result": {
            "context": { "slot": 350_000_000_u64, "apiVersion": "2.1.9" },
            "value": balance,
        },
        "id": 1
    }))
}
