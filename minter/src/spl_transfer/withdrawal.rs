use super::{create_associated_token_account_instruction, transfer_checked_instruction};
use crate::{
    address::associated_token_address,
    sol_transfer::{CreateTransferError, build_nonce_message},
    state::SupportedSplToken,
};
use solana_address::Address;
use solana_hash::Hash;
use solana_message::Message;

/// One transfer from a treasury ATA to a destination wallet's ATA.
pub struct SplWithdrawalTransfer<'a> {
    pub token: &'a SupportedSplToken,
    pub destination: &'a Address,
    pub amount: u64,
}

/// Builds the unsigned message of one SPL withdrawal: nonce advancement first, then the
/// creation of the destination ATA unless it exists, then the transfer from the treasury
/// ATA. The minter pays, owns the treasury and is the nonce authority, so it signs alone.
pub fn build_spl_withdrawal_message(
    minter_address: &Address,
    nonce_account: &Address,
    nonce_value: Hash,
    transfer: &SplWithdrawalTransfer<'_>,
) -> Result<Message, CreateTransferError> {
    let mint = &transfer.token.mint;
    let source = associated_token_address(minter_address, mint);
    let destination = associated_token_address(transfer.destination, mint);
    build_nonce_message(
        minter_address,
        nonce_account,
        nonce_value,
        [
            create_associated_token_account_instruction(minter_address, transfer.destination, mint),
            transfer_checked_instruction(
                &source,
                mint,
                &destination,
                minter_address,
                transfer.amount,
                transfer.token.decimals,
            ),
        ],
    )
}
