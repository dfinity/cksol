use cksol_types::{GetTransactionError, InvalidTransaction};
use sol_rpc_types::{GetTransactionEncoding, Lamport, MultiRpcResult, Signature};
use solana_address::Address;
use solana_transaction_status_client_types::EncodedConfirmedTransactionWithStatusMeta;

// TODO: Read this from state
const MIN_DEPOSIT_THRESHOLD: Lamport = 100_000;

pub async fn try_get_transaction(
    signature: Signature,
) -> Result<Option<EncodedConfirmedTransactionWithStatusMeta>, GetTransactionError> {
    let signature = solana_signature::Signature::from(signature);

    let client = sol_rpc_client::SolRpcClient::builder_for_ic().build();

    let result = client
        .get_transaction(signature)
        .with_encoding(GetTransactionEncoding::Base64)
        .with_cycles(10_000_000_000_000)
        .try_send()
        .await;

    match result {
        Ok(MultiRpcResult::Consistent(Ok(maybe_tx))) => Ok(maybe_tx),
        Ok(MultiRpcResult::Consistent(Err(e))) => Err(GetTransactionError::RpcError(e)),
        Ok(MultiRpcResult::Inconsistent(results)) => Err(GetTransactionError::InconsistentResults(
            results
                .into_iter()
                .map(|(source, result)| {
                    (
                        source,
                        result.and_then(|maybe_tx|
                            maybe_tx.map(sol_rpc_types::EncodedConfirmedTransactionWithStatusMeta::try_from).transpose()
                        ),
                    )
                })
                .collect(),
        )),
        Err(e) => Err(GetTransactionError::IcError(e.to_string())),
    }
}

pub fn get_deposit_amount_to_address(
    transaction: EncodedConfirmedTransactionWithStatusMeta,
    deposit_address: Address,
) -> Result<Option<Lamport>, InvalidTransaction> {
    let message = transaction
        .transaction
        .transaction
        .decode()
        .ok_or(InvalidTransaction::DecodingFailed)?
        .message;
    let account_keys = message.static_account_keys();

    let deposit_address_index = account_keys
        .iter()
        .position(|address| address == &deposit_address)
        .ok_or(InvalidTransaction::NotDepositToAddress)?;
    // TODO: Ensure writable an source is transaction
    if message.is_signer(deposit_address_index) {
        return Err(InvalidTransaction::NotDepositToAddress);
    }

    let meta = transaction
        .transaction
        .meta
        .ok_or(InvalidTransaction::NoTransactionMeta)?;
    let pre_balance = *meta
        .pre_balances
        .get(deposit_address_index)
        .ok_or(InvalidTransaction::DecodingFailed)?;
    let post_balance = *meta
        .post_balances
        .get(deposit_address_index)
        .ok_or(InvalidTransaction::DecodingFailed)?;

    if post_balance >= pre_balance + MIN_DEPOSIT_THRESHOLD {
        Ok(Some(post_balance.saturating_sub(pre_balance)))
    } else {
        Ok(None)
    }
}
