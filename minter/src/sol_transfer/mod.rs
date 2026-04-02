use crate::{
    address::{
        DerivationPath, derivation_path, derive_public_key, lazy_get_schnorr_master_key,
        minter_address,
    },
    constants::SOLANA_LAMPORTS_PER_SIGNATURE,
    runtime::CanisterRuntime,
    signer::{SchnorrSigner, sign_bytes},
};
use derive_more::From;
use ic_cdk_management_canister::SignCallError;
use icrc_ledger_types::icrc1::account::Account;
use sol_rpc_types::Lamport;
use solana_address::Address;
use solana_hash::Hash;
use solana_system_interface::instruction;
use solana_transaction::{Instruction, Message, Transaction};
use std::collections::BTreeMap;
use thiserror::Error;

#[cfg(test)]
mod tests;

pub const MAX_SIGNATURES: u64 = 10;
pub const MAX_TX_SIZE: usize = 1_232;
const BYTES_PER_SIGNATURE: usize = 64;

/// Upper bound on the number of withdrawal transfers that fit in a single
/// Solana transaction when the fee-payer is the only signer.
pub const MAX_WITHDRAWALS_PER_TX: usize = 20;

#[derive(Debug, Error, From)]
pub enum CreateTransferError {
    #[error("transaction size {got} exceeds maximum of {max} bytes")]
    TransactionTooLarge { max: usize, got: usize },
    #[error("signing failed: {0}")]
    SigningFailed(SignCallError),
}

/// Creates a signed Solana transaction that transfers lamports from
/// each minter-controlled source address to the minter's consolidated address.
///
/// The first source account is used as the fee payer.
/// Sources are reduced by account (duplicate accounts have their amounts summed).
///
/// Returns the signed transaction and the list of signer accounts.
///
/// # Panics
///
/// * Panics if `sources` is empty.
/// * Panics if the IC returns a signature that is not exactly 64 bytes.
pub async fn create_signed_consolidation_transaction<R: CanisterRuntime>(
    runtime: &R,
    sources: Vec<(Account, Lamport)>,
    recent_blockhash: Hash,
) -> Result<(Transaction, Vec<Account>), CreateTransferError> {
    let sources: Vec<(Account, Lamport)> = sources
        .into_iter()
        .fold(
            BTreeMap::<Account, Lamport>::new(),
            |mut map, (account, amount)| {
                *map.entry(account).or_default() += &amount;
                map
            },
        )
        .into_iter()
        .collect();
    assert!(!sources.is_empty(), "BUG: sources must not be empty");

    let master_public_key = lazy_get_schnorr_master_key().await;
    let target_address = minter_address(&master_public_key, runtime);
    let (derivation_paths, addresses): (Vec<_>, Vec<_>) = sources
        .iter()
        .map(|(account, _)| {
            let path = derivation_path(account);
            let public_key = derive_public_key(&master_public_key, path.to_vec());
            (path, Address::from(public_key.serialize_raw()))
        })
        .unzip();

    let fee_payer_address = &addresses[0];
    let transaction_fee = SOLANA_LAMPORTS_PER_SIGNATURE * sources.len() as u64;

    let instructions: Vec<Instruction> = addresses
        .iter()
        .zip(&sources)
        .enumerate()
        .map(|(index, (source, (_, amount)))| {
            let transfer_amount = if index == 0 {
                amount
                    .checked_sub(transaction_fee)
                    .expect("BUG: fee payer has insufficient funds to cover the transaction fee")
            } else {
                *amount
            };
            instruction::transfer(source, &target_address, transfer_amount)
        })
        .collect();

    let message =
        Message::new_with_blockhash(&instructions, Some(fee_payer_address), &recent_blockhash);
    let mut transaction = Transaction::new_unsigned(message);

    // Re-order signers to match the order of the message account keys
    let mut signer_map: BTreeMap<Address, (Account, DerivationPath)> = addresses
        .into_iter()
        .zip(
            sources
                .iter()
                .map(|(account, _)| *account)
                .zip(derivation_paths),
        )
        .collect();
    let (signer_accounts, signer_derivation_paths): (Vec<_>, Vec<_>) = transaction
        .message
        .signer_keys()
        .iter()
        .map(|key| {
            signer_map
                .remove(key)
                .expect("BUG: signer key not found in source addresses")
        })
        .unzip();

    sign_transaction(&mut transaction, signer_derivation_paths, &runtime.signer()).await?;

    Ok((transaction, signer_accounts))
}

/// Creates a signed Solana transaction that transfers lamports from a single
/// minter-controlled address (the fee payer) to multiple target addresses.
///
/// Returns the signed transaction and the list of signer accounts
/// (only the fee payer).
///
/// # Panics
///
/// Panics if the IC returns a signature that is not exactly 64 bytes.
pub async fn create_signed_batch_withdrawal_transaction<R: CanisterRuntime>(
    runtime: &R,
    targets: &[(Address, Lamport)],
    recent_blockhash: Hash,
) -> Result<(Transaction, Vec<Account>), CreateTransferError> {
    let fee_payer_account = Account::from(runtime.canister_self());
    let master_public_key = lazy_get_schnorr_master_key().await;
    let fee_payer_derivation_path = derivation_path(&fee_payer_account);
    let fee_payer_address = Address::from(
        derive_public_key(&master_public_key, fee_payer_derivation_path.to_vec()).serialize_raw(),
    );

    let instructions: Vec<Instruction> = targets
        .iter()
        .map(|(target, amount)| instruction::transfer(&fee_payer_address, target, *amount))
        .collect();

    let message =
        Message::new_with_blockhash(&instructions, Some(&fee_payer_address), &recent_blockhash);
    let mut transaction = Transaction::new_unsigned(message);

    sign_transaction(
        &mut transaction,
        vec![fee_payer_derivation_path],
        &runtime.signer(),
    )
    .await?;

    Ok((transaction, vec![fee_payer_account]))
}

// Sign transaction, return error if it exceeds the maximum transaction size.
async fn sign_transaction(
    transaction: &mut Transaction,
    signer_derivation_paths: impl IntoIterator<Item = DerivationPath>,
    signer: &impl SchnorrSigner,
) -> Result<(), CreateTransferError> {
    let message_bytes = transaction.message_data();
    let message_len = message_bytes.len();
    transaction.signatures = sign_bytes(signer_derivation_paths, signer, message_bytes).await?;

    let tx_size = 1 + message_len + transaction.signatures.len() * BYTES_PER_SIGNATURE;
    if tx_size > MAX_TX_SIZE {
        return Err(CreateTransferError::TransactionTooLarge {
            max: MAX_TX_SIZE,
            got: tx_size,
        });
    }

    Ok(())
}
