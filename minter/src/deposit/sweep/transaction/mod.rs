use crate::state::Sweep;
use solana_hash::Hash;
use solana_system_interface::instruction;
use solana_transaction::{Instruction, Message};

#[cfg(test)]
mod tests;

impl Sweep {
    /// The message of the sweep transaction: one transfer per deposit to the minter address,
    /// in the order of the planned transfers, paid for by the fee payer.
    pub fn sweep_message(&self, recent_blockhash: Hash) -> Message {
        let transfers = self.transfers();
        let instructions: Vec<Instruction> = transfers
            .iter()
            .map(|transfer| {
                instruction::transfer(&transfer.from, &self.minter_address(), transfer.amount)
            })
            .collect();
        Message::new_with_blockhash(&instructions, Some(&transfers[0].from), &recent_blockhash)
    }
}
