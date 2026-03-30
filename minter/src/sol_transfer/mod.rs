use crate::{
    address::{DerivationPath, derivation_path, derive_public_key, lazy_get_schnorr_master_key},
    runtime::CanisterRuntime,
    signer::{SchnorrSigner, sign_bytes},
};
use derive_more::From;
use ic_cdk::management_canister::SignCallError;
use icrc_ledger_types::icrc1::account::Account;
use sol_rpc_types::Lamport;
use solana_address::Address;
use solana_hash::Hash;
use solana_system_interface::instruction;
use solana_transaction::{Instruction, Message, Transaction};
use std::{collections::BTreeMap, iter};
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
/// each minter-controlled address (identified by its account)
/// to `target_address` Solana address.
///
/// Returns the signed transaction and the list of signer accounts
/// (in signature order: fee payer first, then sources).
///
/// # Panics
///
/// Panics if the IC returns a signature that is not exactly 64 bytes.
pub async fn create_signed_batch_consolidation_transaction(
    fee_payer_account: Account,
    sources: &[(Account, Lamport)],
    target_address: Address,
    recent_blockhash: Hash,
    signer: &impl SchnorrSigner,
) -> Result<(Transaction, Vec<Account>), CreateTransferError> {
    let master_public_key = lazy_get_schnorr_master_key().await;
    let derive_address = |account: &Account| -> (DerivationPath, Address) {
        let derivation_path = derivation_path(account);
        let public_key = derive_public_key(&master_public_key, derivation_path.to_vec());
        (derivation_path, public_key.serialize_raw().into())
    };

    let (fee_payer_derivation_path, fee_payer_address) = derive_address(&fee_payer_account);

    let (source_derivation_paths, source_addresses): (Vec<_>, Vec<_>) = sources
        .iter()
        .map(|(account, _)| derive_address(account))
        .unzip();

    let instructions: Vec<Instruction> = source_addresses
        .iter()
        .zip(sources)
        .map(|(source, (_, amount))| instruction::transfer(source, &target_address, *amount))
        .collect();

    let message =
        Message::new_with_blockhash(&instructions, Some(&fee_payer_address), &recent_blockhash);
    let mut transaction = Transaction::new_unsigned(message);

    // Build a map with all signer addresses and re-order entries to match the
    // order of the message account keys
    let mut signer_map: BTreeMap<Address, (Account, DerivationPath)> =
        iter::chain(iter::once(fee_payer_address), source_addresses)
            .zip(iter::zip(
                iter::once(fee_payer_account).chain(sources.iter().map(|(account, _)| *account)),
                iter::once(fee_payer_derivation_path).chain(source_derivation_paths),
            ))
            .collect();
    let (signer_accounts, signer_derivation_paths): (Vec<_>, Vec<_>) = transaction
        .message
        .signer_keys()
        .iter()
        .map(|key| {
            signer_map
                .remove(key)
                .expect("BUG: signer key not found in fee payer and source addresses")
        })
        .unzip();

    sign_transaction(&mut transaction, signer_derivation_paths, signer).await?;

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
    if tx_size >= MAX_TX_SIZE {
        return Err(CreateTransferError::TransactionTooLarge {
            max: MAX_TX_SIZE,
            got: tx_size,
        });
    }

    Ok(())
}
