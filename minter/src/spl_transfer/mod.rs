use crate::{
    runtime::CanisterRuntime,
    sol_transfer::{CreateTransferError, sign_transaction},
    state::{SplSweep, event::Signer},
};
use solana_address::Address;
use solana_hash::Hash;
use solana_instruction::Instruction;
use solana_transaction::Transaction;

#[cfg(test)]
mod tests;

/// Builds one classic SPL Token `TransferChecked`, signed by the source owner.
/// Account creation, closing and transaction signing are separate operations.
pub fn transfer_checked_instruction(
    source: &Address,
    mint: &Address,
    destination: &Address,
    authority: &Address,
    amount: u64,
    decimals: u8,
) -> Instruction {
    spl_token_interface::instruction::transfer_checked(
        &spl_token_interface::id(),
        &source.to_bytes().into(),
        &mint.to_bytes().into(),
        &destination.to_bytes().into(),
        &authority.to_bytes().into(),
        &[],
        amount,
        decimals,
    )
    .expect("BUG: transfer_checked only rejects a foreign token program id")
}

/// Signs an SPL sweep with the minter and the deposit owner.
/// Returns the signed transaction and its signers in signature order.
pub async fn sign_spl_sweep_transaction<R: CanisterRuntime>(
    runtime: &R,
    sweep: &SplSweep,
    recent_blockhash: Hash,
) -> Result<(Transaction, Vec<Signer>), CreateTransferError> {
    let mut transaction = Transaction::new_unsigned(sweep.sweep_message(recent_blockhash));
    let signers = sweep.signers().to_vec();
    sign_transaction(
        &mut transaction,
        signers.iter().map(Signer::derivation_path),
        &runtime.signer(),
    )
    .await?;
    Ok((transaction, signers))
}
