use crate::{
    constants::FEE_PER_SIGNATURE,
    state::{QueuedDeposit, event::VersionedMessage},
};
use cksol_types::DepositSolId;
use sol_rpc_types::Lamport;
use solana_address::Address;
use solana_hash::Hash;
use solana_signature::Signature;
use solana_system_interface::instruction;
use solana_transaction::{Instruction, Message};
use std::{cmp::Reverse, collections::BTreeMap};
use thiserror::Error;

#[cfg(test)]
mod tests;

/// The sweep transactions at one stage of the deposit lifecycle, keyed by signature.
///
/// A deposit belongs to exactly one sweep, so it can also be found by its id:
/// [`Sweep::plan`] rejects a duplicated deposit id and [`Sweeps::insert`] rejects
/// a sweep containing a deposit that another sweep already holds.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Sweeps {
    by_signature: BTreeMap<Signature, Sweep>,
}

impl Sweeps {
    pub fn get(&self, signature: &Signature) -> Option<&Sweep> {
        self.by_signature.get(signature)
    }

    /// The sweep of the given deposit, together with the deposit itself.
    pub fn deposit(&self, deposit_id: DepositSolId) -> Option<(&Signature, &QueuedDeposit)> {
        self.by_signature
            .iter()
            .find_map(|(signature, sweep)| Some((signature, sweep.deposits.get(&deposit_id)?)))
    }

    pub fn signatures(&self) -> impl Iterator<Item = &Signature> {
        self.by_signature.keys()
    }

    pub fn len(&self) -> usize {
        self.by_signature.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_signature.is_empty()
    }

    pub fn deposit_count(&self) -> usize {
        self.by_signature.values().map(Sweep::deposit_count).sum()
    }

    pub(super) fn insert(&mut self, signature: Signature, sweep: Sweep) {
        for deposit_id in sweep.deposits().keys() {
            assert!(
                self.deposit(*deposit_id).is_none(),
                "Attempted to record deposit {deposit_id} in sweep {signature} while another sweep holds it"
            );
        }
        assert!(
            self.by_signature.insert(signature, sweep).is_none(),
            "Attempted to record sweep {signature} twice"
        );
    }

    pub(super) fn remove(&mut self, signature: &Signature) -> Option<Sweep> {
        self.by_signature.remove(signature)
    }
}

/// The plan of one sweep transaction: the deposits it moves to the minter's main account,
/// the deposit paying the transaction fee, the fee assumed for it, and the destination.
///
/// The plan is a function of the queued deposits and the destination. The state keeps the
/// plan it recovers from the submitted transaction, after checking that the plan toward the
/// minter address derived from the recorded public key builds that very message, so that
/// the outcome of the transaction can be checked against exactly what the minter signed.
///
/// Each deposit is kept as it was recorded when it was queued, whatever stage the sweep
/// holding it has reached.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sweep {
    deposits: BTreeMap<DepositSolId, QueuedDeposit>,
    fee_payer: DepositSolId,
    fee: Lamport,
    minter_address: Address,
    transfers: Vec<Transfer>,
    expected_received: Lamport,
}

impl Sweep {
    /// Plans the sweep of the given deposits to the minter address. The deposit with the largest
    /// sweepable amount pays the fee of one signature per deposit, so that every deposit address
    /// is left with the rent exemption threshold.
    ///
    /// The transfers are ordered with the fee payer first, then the other deposits by decreasing
    /// sweepable amount. The fee payer's transfer is reduced by the fee so that its address
    /// stays rent-exempt.
    pub fn plan(
        deposits: impl IntoIterator<Item = (DepositSolId, QueuedDeposit)>,
        minter_address: Address,
    ) -> Self {
        let mut unique = BTreeMap::new();
        for (deposit_id, deposit) in deposits {
            assert!(
                unique.insert(deposit_id, deposit).is_none(),
                "Attempted to create a sweep with deposit {deposit_id} twice"
            );
        }
        let (fee_payer, fee_payer_deposit) = unique
            .iter()
            .max_by_key(|(deposit_id, deposit)| (deposit.sweepable_amount(), Reverse(**deposit_id)))
            .map(|(deposit_id, deposit)| (*deposit_id, *deposit))
            .expect("Attempted to plan a sweep without deposits");
        let fee = FEE_PER_SIGNATURE * unique.len() as Lamport;
        let mut others: Vec<_> = unique
            .iter()
            .filter(|(deposit_id, _)| **deposit_id != fee_payer)
            .collect();
        others.sort_by_key(|(deposit_id, deposit)| {
            (Reverse(deposit.sweepable_amount()), **deposit_id)
        });
        let transfers: Vec<_> = std::iter::once(Transfer {
            deposit_id: fee_payer,
            from: fee_payer_deposit.address,
            amount: fee_payer_deposit
                .sweepable_amount()
                .checked_sub(fee)
                .expect("BUG: the minimum deposit amount covers the fee of a full sweep"),
        })
        .chain(others.into_iter().map(|(deposit_id, deposit)| Transfer {
            deposit_id: *deposit_id,
            from: deposit.address,
            amount: deposit.sweepable_amount(),
        }))
        .collect();
        let expected_received = transfers.iter().map(|transfer| transfer.amount).sum();
        Self {
            deposits: unique,
            fee_payer,
            fee,
            minter_address,
            transfers,
            expected_received,
        }
    }

    /// Plans again the sweep of the given deposits to the given minter address and
    /// checks that the plan builds the submitted message, so that a message sweeping
    /// to any other destination is rejected.
    pub fn recover(
        deposits: impl IntoIterator<Item = (DepositSolId, QueuedDeposit)>,
        minter_address: Address,
        submitted: &VersionedMessage,
    ) -> Result<Self, SweepRecoveryError> {
        let VersionedMessage::Legacy(message) = submitted;
        let sweep = Self::plan(deposits, minter_address);
        let planned = VersionedMessage::Legacy(sweep.sweep_message(message.recent_blockhash));
        if planned != *submitted {
            return Err(SweepRecoveryError::UnexpectedMessage {
                planned: Box::new(planned),
                submitted: Box::new(submitted.clone()),
            });
        }
        Ok(sweep)
    }

    pub fn deposits(&self) -> &BTreeMap<DepositSolId, QueuedDeposit> {
        &self.deposits
    }

    pub fn fee_payer(&self) -> DepositSolId {
        self.fee_payer
    }

    pub fn fee(&self) -> Lamport {
        self.fee
    }

    pub fn minter_address(&self) -> Address {
        self.minter_address
    }

    /// The message of the sweep transaction: one transfer per deposit to the minter address,
    /// in the order of the planned transfers, paid for by the fee payer.
    pub fn sweep_message(&self, recent_blockhash: Hash) -> Message {
        let instructions: Vec<Instruction> = self
            .transfers
            .iter()
            .map(|transfer| {
                instruction::transfer(&transfer.from, &self.minter_address, transfer.amount)
            })
            .collect();
        let fee_payer = self.transfers.first().map(|transfer| &transfer.from);
        Message::new_with_blockhash(&instructions, fee_payer, &recent_blockhash)
    }

    /// The transfers of the sweep transaction in their order, the fee payer first.
    pub fn transfers(&self) -> &[Transfer] {
        &self.transfers
    }

    /// The amount the minter address receives: the sweepable amounts minus the fee.
    pub fn expected_received(&self) -> Lamport {
        self.expected_received
    }

    pub fn deposit_count(&self) -> usize {
        self.deposits.len()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum SweepRecoveryError {
    #[error("the plan builds {planned:?} but the submitted message is {submitted:?}")]
    UnexpectedMessage {
        planned: Box<VersionedMessage>,
        submitted: Box<VersionedMessage>,
    },
}

/// One transfer of a sweep transaction, from a deposit address to the minter address.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Transfer {
    pub deposit_id: DepositSolId,
    pub from: Address,
    pub amount: Lamport,
}
