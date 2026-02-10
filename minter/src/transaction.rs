use cksol_types::InvalidDepositTransaction;
use ic_canister_runtime::IcError;
use sol_rpc_client::SolRpcClient;
use sol_rpc_types::{GetTransactionEncoding, Lamport, MultiRpcResult};
use solana_address::Address;
use solana_transaction_status_client_types::EncodedConfirmedTransactionWithStatusMeta;

// TODO: Read this from state
const MIN_DEPOSIT_THRESHOLD: Lamport = 100_000;

pub async fn try_get_transaction(
    signature: impl Into<solana_signature::Signature>,
) -> Result<Option<EncodedConfirmedTransactionWithStatusMeta>, String> {
    let client = SolRpcClient::builder_for_ic().build();

    let result = client
        .get_transaction(signature.into())
        .with_encoding(GetTransactionEncoding::Base64)
        .with_cycles(10_000_000_000_000)
        .try_send()
        .await
        .map_err(|e| match e {
            // The minter canister should never run out of cycles
            e @ IcError::InsufficientLiquidCycleBalance { .. } => panic!("{e:?}"),
            // Candid decode should never fail here
            e @ IcError::CandidDecodeFailed { .. } => panic!("{e:?}"),
            IcError::CallPerformFailed | IcError::CallRejected { .. } => e.to_string(),
        })?;

    match result {
        MultiRpcResult::Consistent(Ok(maybe_tx)) => Ok(maybe_tx),
        MultiRpcResult::Consistent(Err(e)) => Err(e.to_string()),
        MultiRpcResult::Inconsistent(_) => {
            Err("Inconsistent RPC results for `getTransaction`".to_string())
        }
    }
}

pub fn get_deposit_amount_to_address(
    transaction: EncodedConfirmedTransactionWithStatusMeta,
    deposit_address: Address,
) -> Result<Lamport, InvalidDepositTransaction> {
    let message = transaction
        .transaction
        .transaction
        .decode()
        .ok_or(InvalidDepositTransaction::DecodingFailed)?
        .message;

    // Search only static account keys, which guarantees the deposit address
    // is sourced from the transaction itself (not an address lookup table).
    let account_keys = message.static_account_keys();

    let deposit_address_index = account_keys
        .iter()
        .position(|address| address == &deposit_address)
        .ok_or(InvalidDepositTransaction::InvalidTransfer("".to_string()))?;

    // The deposit address must be writable (to receive funds) but must not
    // be a signer (it's controlled by the minter, not the depositor).
    if !message.is_maybe_writable(deposit_address_index, None) {
        return Err(InvalidDepositTransaction::InvalidTransfer(
            "Deposit address must be writable".to_string(),
        ));
    }
    if message.is_signer(deposit_address_index) {
        return Err(InvalidDepositTransaction::InvalidTransfer(
            "Deposit address cannot be a signer".to_string(),
        ));
    }

    let meta = transaction
        .transaction
        .meta
        .ok_or(InvalidDepositTransaction::DecodingFailed)?;
    let pre_balance = *meta
        .pre_balances
        .get(deposit_address_index)
        .ok_or(InvalidDepositTransaction::DecodingFailed)?;
    let post_balance = *meta
        .post_balances
        .get(deposit_address_index)
        .ok_or(InvalidDepositTransaction::DecodingFailed)?;

    let deposit_amount = post_balance.saturating_sub(pre_balance);
    if deposit_amount >= MIN_DEPOSIT_THRESHOLD {
        Ok(deposit_amount)
    } else {
        Err(InvalidDepositTransaction::InsufficientDepositAmount {
            received: deposit_amount,
            minimum: MIN_DEPOSIT_THRESHOLD,
        })
    }
}
