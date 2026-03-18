use crate::{
    address::{DerivationPath, derivation_path, derive_public_key, lazy_get_schnorr_master_key},
    state::read_state,
};
use derive_more::From;
use ic_cdk::management_canister::{
    SchnorrAlgorithm, SchnorrKeyId, SignCallError, SignWithSchnorrArgs, sign_with_schnorr,
};
use icrc_ledger_types::icrc1::account::Account;
use indexmap::IndexSet;
use sol_rpc_types::Lamport;
use solana_address::Address;
use solana_hash::Hash;
use solana_signature::Signature;
use solana_system_interface::instruction;
use solana_transaction::{Instruction, Message, Transaction};
use std::iter;
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
    // Order signers in the order their signatures should be included in the serialized transaction.
    // The fee payer is always at index 0, followed by deduplicated signers for each instruction.
    let signer_accounts: IndexSet<_> = iter::once(&fee_payer_account)
        .chain(sources.iter().map(|(account, _)| account))
        .collect();
    if signer_accounts.len() as u64 > MAX_SIGNATURES {
        return Err(CreateTransferError::TooManySignatures {
            max: MAX_SIGNATURES,
            got: signer_accounts.len() as u64,
        });
    }

    let master_public_key = lazy_get_schnorr_master_key().await;
    let derive_address = |account: &Account| -> Address {
        Address::from(
            derive_public_key(&master_public_key, derivation_path(account)).serialize_raw(),
        )
    };

    let target_address = derive_address(&destination_account);
    let fee_payer_address = derive_address(&fee_payer_account);

    let instructions: Vec<Instruction> = sources
        .iter()
        .map(|(account, amount)| {
            instruction::transfer(&derive_address(account), &target_address, *amount)
        })
        .collect();

    let message =
        Message::new_with_blockhash(&instructions, Some(&fee_payer_address), &recent_blockhash);
    let mut transaction = Transaction::new_unsigned(message);
    let message_bytes = transaction.message_data();

    // Check serialized transaction size does not exceed maximum Solana transaction size:
    assert!(1 + message_bytes.len() + signer_accounts.len() * BYTES_PER_SIGNATURE < MAX_TX_SIZE);

    let signatures = futures::future::try_join_all(
        signer_accounts
            .iter()
            .map(|account| signer.sign(message_bytes.clone(), derivation_path(account))),
    )
    .await?
    .into_iter()
    .map(signature_from_bytes)
    .collect();

    transaction.signatures = signatures;

    let signers = signer_accounts.into_iter().cloned().collect();
    Ok((transaction, signers))
}

fn signature_from_bytes(bytes: Vec<u8>) -> Signature {
    <[u8; 64]>::try_from(bytes.as_slice())
        .unwrap_or_else(|_| panic!("BUG: expected 64-byte signature, got {} bytes", bytes.len()))
        .into()
}
