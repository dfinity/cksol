use crate::{
    address::{account_address, associated_token_address, minter_address},
    constants::FEE_PER_SIGNATURE,
    spl_transfer::transfer_checked_instruction,
    state::{
        QueuedSplDeposit, SchnorrPublicKey, SupportedSplToken,
        event::{Signer, VersionedMessage},
    },
};
use cksol_types::DepositSplId;
use sol_rpc_types::Lamport;
use solana_address::Address;
use solana_hash::Hash;
use solana_signature::Signature;
use solana_transaction::Message;
use std::collections::BTreeMap;
use thiserror::Error;

#[cfg(test)]
mod tests;

/// The network fee of one SPL sweep: the signatures of the minter and of the deposit owner.
pub const SPL_SWEEP_TRANSACTION_FEE: Lamport = 2 * FEE_PER_SIGNATURE;

/// The SPL sweep transactions at one stage of the deposit lifecycle, keyed by signature.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SplSweeps {
    by_signature: BTreeMap<Signature, SubmittedSplSweep>,
}

impl SplSweeps {
    pub fn by_signature(&self) -> &BTreeMap<Signature, SubmittedSplSweep> {
        &self.by_signature
    }

    pub fn get(&self, signature: &Signature) -> Option<&SubmittedSplSweep> {
        self.by_signature.get(signature)
    }

    /// The sweep of the given deposit, together with the deposit itself.
    pub fn deposit(&self, deposit_id: DepositSplId) -> Option<(&Signature, &QueuedSplDeposit)> {
        self.by_signature.iter().find_map(|(signature, submitted)| {
            (submitted.sweep.deposit_id == deposit_id)
                .then_some((signature, &submitted.sweep.deposit))
        })
    }

    pub fn signatures(&self) -> impl Iterator<Item = &Signature> {
        self.by_signature.keys()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&Signature, &SubmittedSplSweep)> {
        self.by_signature.iter()
    }

    pub fn len(&self) -> usize {
        self.by_signature.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_signature.is_empty()
    }

    pub(super) fn insert(&mut self, signature: Signature, sweep: SubmittedSplSweep) {
        assert!(
            self.by_signature.insert(signature, sweep).is_none(),
            "Attempted to record SPL sweep {signature} twice"
        );
    }

    pub(super) fn remove(&mut self, signature: &Signature) -> Option<SubmittedSplSweep> {
        self.by_signature.remove(signature)
    }
}

/// A submitted sweep with the SOL withheld for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubmittedSplSweep {
    pub sweep: SplSweep,
    /// SOL withheld from both spendable balances until this sweep is reconciled.
    pub reserved_lamports: Lamport,
}

/// The plan of one SPL sweep: one deposit transferred into its pre-created treasury ATA,
/// with the minter as the fee payer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SplSweep {
    deposit_id: DepositSplId,
    deposit: QueuedSplDeposit,
    minter_address: Address,
    owner: Address,
    decimals: u8,
}

impl SplSweep {
    pub fn plan(
        deposit_id: DepositSplId,
        deposit: QueuedSplDeposit,
        token: &SupportedSplToken,
        master_key: &SchnorrPublicKey,
    ) -> Result<Self, SplSweepRecoveryError> {
        if token.mint != deposit.mint
            || token.token_program.to_bytes() != spl_token_interface::id().to_bytes()
        {
            return Err(SplSweepRecoveryError::MintMismatch);
        }
        let owner = account_address(master_key, &deposit.account);
        if associated_token_address(&owner, &deposit.mint) != deposit.address {
            return Err(SplSweepRecoveryError::SourceMismatch { deposit_id });
        }
        Ok(Self {
            deposit_id,
            deposit,
            minter_address: minter_address(master_key),
            owner,
            decimals: token.decimals,
        })
    }

    /// Reconstructs the plan from the immutable registration and the queued terms, and
    /// checks that it builds the submitted message with the recorded signers.
    pub fn recover(
        deposit_id: DepositSplId,
        deposit: QueuedSplDeposit,
        token: &SupportedSplToken,
        master_key: &SchnorrPublicKey,
        submitted: &VersionedMessage,
        signers: &[Signer],
    ) -> Result<Self, SplSweepRecoveryError> {
        let sweep = Self::plan(deposit_id, deposit, token, master_key)?;
        if signers != sweep.signers() {
            return Err(SplSweepRecoveryError::InvalidSigners);
        }
        let VersionedMessage::Legacy(message) = submitted;
        let planned = sweep.sweep_message(message.recent_blockhash);
        if planned != *message {
            return Err(SplSweepRecoveryError::UnexpectedMessage {
                planned: Box::new(planned.into()),
                submitted: Box::new(submitted.clone()),
            });
        }
        Ok(sweep)
    }

    pub fn deposit_id(&self) -> DepositSplId {
        self.deposit_id
    }

    pub fn deposit(&self) -> &QueuedSplDeposit {
        &self.deposit
    }

    /// The signers in signature order: the minter as the fee payer, then the deposit owner.
    pub fn signers(&self) -> [Signer; 2] {
        [Signer::Minter, Signer::Account(self.deposit.account)]
    }

    pub fn sweep_message(&self, blockhash: Hash) -> Message {
        let transfer = transfer_checked_instruction(
            &self.deposit.address,
            &self.deposit.mint,
            &self.treasury_address(),
            &self.owner,
            self.deposit.balance,
            self.decimals,
        );
        Message::new_with_blockhash(&[transfer], Some(&self.minter_address), &blockhash)
    }

    fn treasury_address(&self) -> Address {
        associated_token_address(&self.minter_address, &self.deposit.mint)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum SplSweepRecoveryError {
    #[error("the recorded signers do not match the SPL sweep")]
    InvalidSigners,
    #[error("BUG: SPL sweep mint mismatch")]
    MintMismatch,
    #[error("BUG: SPL sweep source mismatch for deposit {deposit_id}")]
    SourceMismatch { deposit_id: DepositSplId },
    #[error("the plan builds {planned:?} but the submitted message is {submitted:?}")]
    UnexpectedMessage {
        planned: Box<VersionedMessage>,
        submitted: Box<VersionedMessage>,
    },
}
