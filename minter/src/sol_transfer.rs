use crate::{address::derive_public_key, state::SchnorrPublicKey};
use cksol_types_internal::Ed25519KeyName;
use ic_cdk::management_canister::{
    SchnorrAlgorithm, SchnorrKeyId, SignCallError, SignWithSchnorrArgs, SignWithSchnorrResult,
    sign_with_schnorr,
};
use sol_rpc_types::Lamport;
use solana_address::Address;
use solana_hash::Hash;
use solana_signature::Signature;
use solana_transaction::{AccountMeta, Instruction, Message, Transaction};

#[cfg(test)]
mod tests;

/// The Solana System Program address (all zero bytes).
const SYSTEM_PROGRAM_ID: Address = Address::new_from_array([0u8; 32]);

pub trait SchnorrSigner {
    fn sign(
        &self,
        args: &SignWithSchnorrArgs,
    ) -> impl std::future::Future<Output = Result<SignWithSchnorrResult, SignCallError>>;
}

/// Production signer that delegates to the IC management canister.
pub struct IcSchnorrSigner;

impl SchnorrSigner for IcSchnorrSigner {
    async fn sign(
        &self,
        args: &SignWithSchnorrArgs,
    ) -> Result<SignWithSchnorrResult, SignCallError> {
        sign_with_schnorr(args).await
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
    key_name: Ed25519KeyName,
    sources: &[(Vec<Vec<u8>>, Lamport)],
    target_address: Address,
    recent_blockhash: Hash,
    signer: &impl SchnorrSigner,
) -> Result<Transaction, SignCallError> {
    assert!(!sources.is_empty(), "BUG: sources must not be empty");

    let source_addresses: Vec<Address> = sources
        .iter()
        .map(|(path, _)| derive_public_key(master_public_key, path.to_vec()))
        .map(|public_key| Address::from(public_key.serialize_raw()))
        .collect();

    let fee_payer = source_addresses[0];

    let instructions: Vec<Instruction> = source_addresses
        .iter()
        .zip(sources)
        .map(|(source, (_, amount))| system_transfer_instruction(source, &target_address, *amount))
        .collect();

    let message = Message::new_with_blockhash(&instructions, Some(&fee_payer), &recent_blockhash);
    let mut transaction = Transaction::new_unsigned(message);
    let message_bytes = transaction.message_data();

    let sign_args: Vec<_> = sources
        .iter()
        .map(|(derivation_path, _)| SignWithSchnorrArgs {
            message: message_bytes.clone(),
            derivation_path: derivation_path.clone(),
            key_id: SchnorrKeyId {
                algorithm: SchnorrAlgorithm::Ed25519,
                name: key_name.to_string(),
            },
            aux: None,
        })
        .collect();

    let results = futures::future::join_all(sign_args.iter().map(|args| signer.sign(args))).await;

    for (i, result) in results.into_iter().enumerate() {
        let response = result?;

        let sig_bytes: [u8; 64] = response
            .signature
            .as_slice()
            .try_into()
            .unwrap_or_else(|_| {
                panic!(
                    "BUG: expected 64-byte signature, got {} bytes",
                    response.signature.len()
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

/// Creates a Solana System Program transfer instruction.
///
/// The instruction data is the bincode encoding of `SystemInstruction::Transfer { lamports }`:
/// 4 bytes (u32 LE) variant index `2` + 8 bytes (u64 LE) lamports.
fn system_transfer_instruction(from: &Address, to: &Address, lamports: Lamport) -> Instruction {
    let mut data = Vec::with_capacity(12);
    data.extend_from_slice(&2u32.to_le_bytes());
    data.extend_from_slice(&lamports.to_le_bytes());

    Instruction {
        program_id: SYSTEM_PROGRAM_ID,
        accounts: vec![AccountMeta::new(*from, true), AccountMeta::new(*to, false)],
        data,
    }
}
