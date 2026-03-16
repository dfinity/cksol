use crate::{
    address::{DerivationPath, derivation_path, derive_public_key, lazy_get_schnorr_master_key},
    state::read_state,
};
use derive_more::From;
use ic_cdk::management_canister::{
    SchnorrAlgorithm, SchnorrKeyId, SignCallError, SignWithSchnorrArgs, sign_with_schnorr,
};
use icrc_ledger_types::icrc1::account::Account;
use sol_rpc_types::Lamport;
use solana_address::Address;
use solana_hash::Hash;
use solana_signature::Signature;
use solana_system_interface::instruction;
use solana_transaction::{Instruction, Message, Transaction};
use std::collections::BTreeSet;
use thiserror::Error;

pub const MAX_SIGNATURES: u64 = 10;
pub const MAX_TX_SIZE: usize = 1_232;
const BYTES_PER_SIGNATURE: usize = 64;

#[derive(Debug, Error, From)]
pub enum CreateTransferError {
    #[error("too many signatures: got {got}, max is {max}")]
    TooManySignatures { max: u64, got: u64 },
    #[error("signing failed: {0}")]
    SigningFailed(SignCallError),
}

#[cfg(test)]
mod tests;

pub trait SchnorrSigner {
    fn sign(
        &self,
        message: Vec<u8>,
        derivation_path: DerivationPath,
    ) -> impl Future<Output = Result<Vec<u8>, SignCallError>>;
}

/// Production signer that delegates to the IC management canister.
pub struct IcSchnorrSigner;

impl SchnorrSigner for IcSchnorrSigner {
    async fn sign(
        &self,
        message: Vec<u8>,
        derivation_path: DerivationPath,
    ) -> Result<Vec<u8>, SignCallError> {
        let key_name = read_state(|s| s.master_key_name());
        let args = SignWithSchnorrArgs {
            message,
            derivation_path,
            key_id: SchnorrKeyId {
                algorithm: SchnorrAlgorithm::Ed25519,
                name: key_name.to_string(),
            },
            aux: None,
        };
        let response = sign_with_schnorr(&args).await?;
        Ok(response.signature)
    }
}

/// Creates a signed Solana transaction that transfers lamports from
/// each minter-controlled address (identified by its account) to the
/// destination account's derived address.
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
) -> Result<Transaction, CreateTransferError> {
    let master_public_key = lazy_get_schnorr_master_key().await;

    let derive_address = |account: &Account| -> (DerivationPath, Address) {
        let derivation_path = derivation_path(account);
        let public_key = derive_public_key(&master_public_key, derivation_path.to_vec());
        (derivation_path, Address::from(public_key.serialize_raw()))
    };

    let (_, target_address) = derive_address(&destination_account);
    let (fee_payer_derivation_path, fee_payer_address) = derive_address(&fee_payer_account);

    let (source_derivation_paths, source_addresses): (Vec<_>, Vec<_>) = sources
        .iter()
        .map(|(account, _)| derive_address(account))
        .unzip();

    // Ensure source accounts are unique
    let unique_source_addresses = source_addresses.iter().collect::<BTreeSet<_>>();
    assert_eq!(
        unique_source_addresses.len(),
        source_addresses.len(),
        "BUG: source accounts must be unique"
    );

    // Add fee payer account to signers if it is not also a source account
    let is_fee_payer_in_sources = unique_source_addresses.contains(&fee_payer_address);
    let (mut signer_derivation_paths, mut signer_addresses) =
        (source_derivation_paths, source_addresses.clone());
    if !is_fee_payer_in_sources {
        signer_derivation_paths.push(fee_payer_derivation_path);
        signer_addresses.push(fee_payer_address);
    }

    // Check signature count after determining unique signers
    let num_signatures = signer_addresses.len() as u64;
    if num_signatures > MAX_SIGNATURES {
        return Err(CreateTransferError::TooManySignatures {
            max: MAX_SIGNATURES,
            got: num_signatures,
        });
    }

    let instructions: Vec<Instruction> = source_addresses
        .iter()
        .zip(sources)
        .map(|(source, (_, amount))| instruction::transfer(source, &target_address, *amount))
        .collect();

    let message =
        Message::new_with_blockhash(&instructions, Some(&fee_payer_address), &recent_blockhash);
    let mut transaction = Transaction::new_unsigned(message);
    let message_bytes = transaction.message_data();

    // message_size + signature_count * signature_size should not exceed tx size limit.
    assert!(message_bytes.len() + signer_addresses.len() * BYTES_PER_SIGNATURE < MAX_TX_SIZE);

    let results = futures::future::join_all(
        signer_derivation_paths
            .iter()
            .map(|derivation_path| signer.sign(message_bytes.clone(), derivation_path.clone())),
    )
    .await;

    for (i, result) in results.into_iter().enumerate() {
        let signature = result?;

        let sig_bytes: [u8; 64] = signature.as_slice().try_into().unwrap_or_else(|_| {
            panic!(
                "BUG: expected 64-byte signature, got {} bytes",
                signature.len()
            )
        });

        let position = transaction
            .message
            .account_keys
            .iter()
            .position(|key| *key == signer_addresses[i])
            .expect("BUG: signer address not found in message account keys");

        transaction.signatures[position] = Signature::from(sig_bytes);
    }

    Ok(transaction)
}
