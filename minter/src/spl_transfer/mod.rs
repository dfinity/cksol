use solana_address::Address;
use solana_instruction::Instruction;

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
