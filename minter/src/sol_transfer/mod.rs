use crate::{
    address::{DerivationPath, MINTER_DERIVATION_PATH},
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
use solana_transaction::{Message, Transaction};
use std::collections::BTreeMap;
use thiserror::Error;

#[cfg(test)]
mod tests;

pub const MAX_SIGNATURES: u64 = 10;
pub const MAX_TX_SIZE: usize = 1_232;
const BYTES_PER_SIGNATURE: usize = 64;

/// Maximum number of withdrawal transfers batched into a single
/// durable-nonce transaction.
pub const MAX_WITHDRAWALS_PER_NONCE_TX: usize = 10;

/// Fee charged for a batch withdrawal transaction, which is signed by the fee payer only.
pub const BATCH_WITHDRAWAL_TX_FEE: Lamport = FEE_PER_SIGNATURE;

#[derive(Debug, Error, From)]
pub enum CreateTransferError {
    #[error("transaction size {got} exceeds maximum of {max} bytes")]
    TransactionTooLarge { max: usize, got: usize },
    #[error("signing failed: {0}")]
    SigningFailed(SignCallError),
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

/// Builds the unsigned message of a batch withdrawal transaction: an
/// `AdvanceNonceAccount` instruction first, followed by one transfer from the
/// minter's main address per withdrawal request, carrying the nonce value in
/// place of a recent blockhash. The main address is the fee payer, the source
/// of all transfers, and the nonce authority, so the transaction has a single
/// signature.
pub fn build_batch_withdrawal_message(
    minter_address: &Address,
    nonce_account: &Address,
    nonce_value: Hash,
    transfers: &[(Address, Lamport)],
) -> Result<Message, CreateTransferError> {
    let mut instructions = vec![instruction::advance_nonce_account(
        nonce_account,
        minter_address,
    )];
    instructions.extend(
        transfers
            .iter()
            .map(|(target, amount)| instruction::transfer(minter_address, target, *amount)),
    );
    let message = Message::new_with_blockhash(&instructions, Some(minter_address), &nonce_value);
    ensure_within_transaction_size(&message)?;
    Ok(message)
}

/// Signs the given withdrawal message with the minter's master key.
pub async fn sign_batch_withdrawal_message<R: CanisterRuntime>(
    runtime: &R,
    message: Message,
) -> Result<Transaction, CreateTransferError> {
    let mut transaction = Transaction::new_unsigned(message);
    sign_transaction(
        &mut transaction,
        [MINTER_DERIVATION_PATH],
        &runtime.signer(),
    )
    .await?;
    Ok(transaction)
}

// Sign transaction, return error if it exceeds the maximum transaction size.
async fn sign_transaction(
    transaction: &mut Transaction,
    signer_derivation_paths: impl IntoIterator<Item = DerivationPath>,
    signer: &impl SchnorrSigner,
) -> Result<(), CreateTransferError> {
    ensure_within_transaction_size(&transaction.message)?;
    transaction.signatures =
        sign_bytes(signer_derivation_paths, signer, transaction.message_data()).await?;
    Ok(())
}

fn ensure_within_transaction_size(message: &Message) -> Result<(), CreateTransferError> {
    let tx_size = 1
        + message.serialize().len()
        + message.header.num_required_signatures as usize * BYTES_PER_SIGNATURE;
    if tx_size > MAX_TX_SIZE {
        return Err(CreateTransferError::TransactionTooLarge {
            max: MAX_TX_SIZE,
            got: tx_size,
        });
    }
    Ok(())
}
