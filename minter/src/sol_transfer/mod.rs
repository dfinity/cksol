use crate::{
    address::{DerivationPath, derivation_path, derive_public_key, lazy_get_schnorr_master_key},
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

pub const MAX_TRANSFERS: usize = 10;
pub const MAX_TX_SIZE: usize = 1_232;
const BYTES_PER_SIGNATURE: usize = 64;

#[derive(Debug, Error, From)]
pub enum CreateTransferError {
    #[error("too many transfers: got {got}, max is {max}")]
    TooManyTransfers { max: usize, got: usize },
    #[error("signing failed: {0}")]
    SigningFailed(SignCallError),
}

#[cfg(test)]
mod tests;

/// Creates a signed Solana transaction that transfers lamports from
/// each minter-controlled address (identified by its account) to the
/// destination account's derived address.
///
/// Returns the signed transaction and the list of signer accounts
/// (in signature order: fee payer first, then sources).
///
/// # Panics
///
/// Panics if the IC returns a signature that is not exactly 64 bytes.
pub async fn create_signed_transfer_transaction(
    fee_payer_account: Account,
    sources: &[(Account, Lamport)],
    destination_account: Account,
    recent_blockhash: Hash,
    signer: &impl SchnorrSigner,
) -> Result<(Transaction, Vec<Account>), CreateTransferError> {
    let master_public_key = lazy_get_schnorr_master_key().await;
    let derive_address = |account: &Account| -> (DerivationPath, Address) {
        let derivation_path = derivation_path(account);
        let public_key = derive_public_key(&master_public_key, derivation_path.to_vec());
        (derivation_path, public_key.serialize_raw().into())
    };

    let (_, target_address) = derive_address(&destination_account);
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
    let message_bytes = transaction.message_data();

    if sources.len() > MAX_TRANSFERS {
        return Err(CreateTransferError::TooManyTransfers {
            max: MAX_TRANSFERS,
            got: sources.len(),
        });
    }

    let num_signers = transaction.message.signer_keys().len();
    let total_size = 1 + message_bytes.len() + num_signers * BYTES_PER_SIGNATURE;
    assert!(
        total_size <= MAX_TX_SIZE,
        "Transaction size {total_size} exceeds maximum {MAX_TX_SIZE} \
         (message={}, signers={num_signers}, accounts={})",
        message_bytes.len(),
        transaction.message.account_keys.len()
    );

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

    transaction.signatures = sign_bytes(signer_derivation_paths, signer, message_bytes).await?;

    Ok((transaction, signer_accounts))
}
