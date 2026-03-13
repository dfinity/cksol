use crate::{
    address::{DerivationPath, derivation_path, derive_public_key},
    state::{SchnorrPublicKey, read_state},
};
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
use std::fmt;

pub const MAX_SOURCES: u64 = 10;
pub const MAX_TX_SIZE: usize = 1_232;

#[derive(Debug)]
pub enum CreateTransferError {
    TooManySources { max: u64, got: u64 },
    SigningFailed(SignCallError),
}

impl fmt::Display for CreateTransferError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooManySources { max, got } => {
                write!(f, "too many sources: got {got}, max is {max}")
            }
            Self::SigningFailed(err) => write!(f, "signing failed: {err}"),
        }
    }
}

impl From<SignCallError> for CreateTransferError {
    fn from(err: SignCallError) -> Self {
        Self::SigningFailed(err)
    }
}

#[cfg(test)]
mod tests;

pub trait SchnorrSigner {
    fn sign(
        &self,
        message: Vec<u8>,
        derivation_path: DerivationPath,
    ) -> impl std::future::Future<Output = Result<Vec<u8>, SignCallError>>;
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

/// Creates a signed Solana transaction that transfers lamports
/// from each minter-controlled address (identified by its derivation path)
/// to the `target_address`.
///
/// The first source address is used as the fee payer.
///
/// # Panics
///
/// Panics if `sources` is empty or if the IC returns a signature
/// that is not exactly 64 bytes.
pub async fn create_signed_transfer_transaction(
    master_public_key: &SchnorrPublicKey,
    sources: &[(Account, Lamport)],
    target_address: Address,
    recent_blockhash: Hash,
    signer: &impl SchnorrSigner,
) -> Result<Transaction, CreateTransferError> {
    assert!(!sources.is_empty(), "BUG: sources must not be empty");

    if sources.len() as u64 > MAX_SOURCES {
        return Err(CreateTransferError::TooManySources {
            max: MAX_SOURCES,
            got: sources.len() as u64,
        });
    }

    let derivation_paths: Vec<DerivationPath> = sources
        .iter()
        .map(|(account, _)| derivation_path(account))
        .collect();

    let source_addresses: Vec<Address> = derivation_paths
        .iter()
        .map(|path| derive_public_key(master_public_key, path.to_vec()))
        .map(|public_key| Address::from(public_key.serialize_raw()))
        .collect();

    let fee_payer = source_addresses[0];

    let instructions: Vec<Instruction> = source_addresses
        .iter()
        .zip(sources)
        .map(|(source, (_, amount))| instruction::transfer(source, &target_address, *amount))
        .collect();

    let message = Message::new_with_blockhash(&instructions, Some(&fee_payer), &recent_blockhash);
    let mut transaction = Transaction::new_unsigned(message);
    let message_bytes = transaction.message_data();
    // message_size + signature_count * signature_size should not exceed tx size limit.
    assert!(message_bytes.len() + sources.len() * 64 < MAX_TX_SIZE);

    let results = futures::future::join_all(
        derivation_paths
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
            .position(|key| *key == source_addresses[i])
            .expect("BUG: signer address not found in message account keys");

        transaction.signatures[position] = Signature::from(sig_bytes);
    }

    Ok(transaction)
}
