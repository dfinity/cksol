use crate::{
    address::{DerivationPath, MinterPublicKeyNotYetAvailable, minter_address, minter_public_key},
    constants::FEE_PER_SIGNATURE,
    runtime::CanisterRuntime,
    signer::{SchnorrSigner, sign_bytes},
    state::{Sweep, event::Signer},
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

/// Fee charged for a batch withdrawal transaction, which is signed by the fee payer only.
pub const BATCH_WITHDRAWAL_TX_FEE: Lamport = FEE_PER_SIGNATURE;

#[derive(Debug, Error, From)]
pub enum CreateTransferError {
    #[error("transaction size {got} exceeds maximum of {max} bytes")]
    TransactionTooLarge { max: usize, got: usize },
    #[error("signing failed: {0}")]
    SigningFailed(SignCallError),
    #[error(transparent)]
    MinterPublicKeyNotYetAvailable(MinterPublicKeyNotYetAvailable),
}

/// Signs the transaction of a planned sweep with the deposit addresses it transfers from.
///
/// Returns the signed transaction and the signer accounts in the order of the signatures.
pub async fn sign_sweep_transaction<R: CanisterRuntime>(
    runtime: &R,
    sweep: &Sweep,
    recent_blockhash: Hash,
) -> Result<(Transaction, Vec<Signer>), CreateTransferError> {
    let mut transaction = Transaction::new_unsigned(sweep.sweep_message(recent_blockhash));
    let accounts_by_address: BTreeMap<Address, Account> = sweep
        .deposits()
        .values()
        .map(|deposit| (deposit.address, deposit.account))
        .collect();
    let signers: Vec<Signer> = transaction
        .message
        .signer_keys()
        .iter()
        .map(|key| {
            let account = accounts_by_address.get(key).copied().unwrap_or_else(|| {
                panic!("BUG: signer {key} is not a deposit address of the sweep")
            });
            Signer::Account(account)
        })
        .collect();

    sign_transaction(
        &mut transaction,
        signers.iter().map(Signer::derivation_path),
        &runtime.signer(),
    )
    .await?;

    Ok((transaction, signers))
}

/// Creates a signed Solana transaction that transfers lamports from a single
/// minter-controlled address (the fee payer) to multiple target addresses.
///
/// Returns the signed transaction and its signers:
/// only [`Signer::Minter`], the fee payer.
///
/// # Panics
///
/// Panics if the IC returns a signature that is not exactly 64 bytes.
pub async fn create_signed_batch_withdrawal_transaction<R: CanisterRuntime>(
    runtime: &R,
    targets: &[(Address, Lamport)],
    recent_blockhash: Hash,
) -> Result<(Transaction, Vec<Signer>), CreateTransferError> {
    let master_public_key = minter_public_key()?;
    let fee_payer_address = minter_address(&master_public_key);

    let instructions: Vec<Instruction> = targets
        .iter()
        .map(|(target, amount)| instruction::transfer(&fee_payer_address, target, *amount))
        .collect();

    let message =
        Message::new_with_blockhash(&instructions, Some(&fee_payer_address), &recent_blockhash);
    let mut transaction = Transaction::new_unsigned(message);

    let signers = vec![Signer::Minter];
    let derivation_paths: Vec<DerivationPath> =
        signers.iter().map(Signer::derivation_path).collect();
    sign_transaction(&mut transaction, derivation_paths, &runtime.signer()).await?;

    Ok((transaction, signers))
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
