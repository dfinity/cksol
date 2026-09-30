use crate::{
    constants::{FEE_PER_SIGNATURE, RENT_EXEMPTION_THRESHOLD},
    state::{
        QueuedDeposit,
        event::{CreditedDeposit, VersionedMessage},
    },
};
use cksol_types::DepositSolId;
use derive_more::From;
use sol_rpc_types::Lamport;
use solana_address::Address;
use solana_hash::Hash;
use solana_signature::Signature;
use solana_system_interface::instruction;
use solana_transaction::{Instruction, Message};
use solana_transaction_status_client_types::EncodedConfirmedTransactionWithStatusMeta;
use std::{cmp::Reverse, collections::BTreeMap};
use thiserror::Error;

#[cfg(test)]
mod tests;

/// The sweep transactions at one stage of the deposit lifecycle, keyed by signature.
///
/// A deposit belongs to exactly one sweep, so it can also be found by its id:
/// [`Sweep::plan`] rejects a duplicated deposit id and [`Sweeps::insert`] rejects
/// a sweep containing a deposit that another sweep already holds.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
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

    pub fn iter(&self) -> impl Iterator<Item = (&Signature, &Sweep)> {
        self.by_signature.iter()
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
/// The plan is a function of the queued deposits alone. The state keeps the plan it recovers
/// from the submitted transaction, after checking that the plan builds that very message, so
/// that the outcome of the transaction can be checked against exactly what the minter signed.
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

    /// Plans again the sweep of the given deposits that the submitted message sweeps,
    /// to the destination of its first transfer, and checks that the plan builds the
    /// submitted message.
    pub fn recover(
        deposits: impl IntoIterator<Item = (DepositSolId, QueuedDeposit)>,
        submitted: &VersionedMessage,
    ) -> Result<Self, SweepRecoveryError> {
        let VersionedMessage::Legacy(message) = submitted;
        let minter_address = message
            .instructions
            .first()
            .and_then(|transfer| transfer.accounts.get(1))
            .and_then(|index| message.account_keys.get(*index as usize))
            .copied()
            .ok_or(SweepRecoveryError::MissingDestination)?;
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

    /// Checks the `getTransaction` output of the sweep against the plan and returns what to
    /// credit, or why the outcome cannot be trusted.
    ///
    /// Nothing is inferred from the outcome: the executed message must be the planned one,
    /// every deposit address must have decreased by exactly its transfer, plus the fee for the
    /// fee payer, and stay rent-exempt, and the minter address must have received exactly the
    /// planned amount. Solana may charge less than the planned fee, which leaves the difference
    /// on the fee payer's address.
    pub fn settle(
        self,
        outcome: &EncodedConfirmedTransactionWithStatusMeta,
    ) -> Result<SettledSweep, SweepSettlementError> {
        let transaction = outcome
            .transaction
            .transaction
            .decode()
            .ok_or(UnreadableOutcome::TransactionDecodingFailed)?;
        let meta = outcome
            .transaction
            .meta
            .as_ref()
            .ok_or(UnreadableOutcome::NoMetaField)?;
        if let Some(error) = &meta.err {
            return Err(SweepMismatch::TransactionFailed {
                error: error.to_string(),
            }
            .into());
        }
        let solana_message::VersionedMessage::Legacy(message) = &transaction.message else {
            return Err(SweepMismatch::UnexpectedMessage.into());
        };
        if message != &self.sweep_message(message.recent_blockhash) {
            return Err(SweepMismatch::UnexpectedMessage.into());
        }
        if meta.pre_balances.len() != message.account_keys.len()
            || meta.post_balances.len() != message.account_keys.len()
        {
            return Err(UnreadableOutcome::IncompleteBalances.into());
        }
        if meta.fee > self.fee() {
            return Err(SweepMismatch::UnexpectedFee {
                expected: self.fee(),
                actual: meta.fee,
            }
            .into());
        }

        let balances: BTreeMap<Address, (Lamport, Lamport)> = message
            .account_keys
            .iter()
            .copied()
            .zip(
                meta.pre_balances
                    .iter()
                    .copied()
                    .zip(meta.post_balances.iter().copied()),
            )
            .collect();
        let balance_of = |address: &Address| {
            balances
                .get(address)
                .copied()
                .ok_or(SweepMismatch::UnexpectedMessage)
        };
        for transfer in self.transfers() {
            let (pre, post) = balance_of(&transfer.from)?;
            let fee_paid = if transfer.deposit_id == self.fee_payer() {
                meta.fee
            } else {
                0
            };
            let expected_decrease = transfer.amount + fee_paid;
            if pre.checked_sub(post) != Some(expected_decrease) {
                return Err(SweepMismatch::UnexpectedBalanceChange {
                    address: transfer.from,
                    pre,
                    post,
                    expected_decrease,
                }
                .into());
            }
            if post < RENT_EXEMPTION_THRESHOLD {
                return Err(SweepMismatch::NotRentExempt {
                    address: transfer.from,
                    post,
                }
                .into());
            }
        }
        let (pre, post) = balance_of(&self.minter_address())?;
        let amount_received = post
            .checked_sub(pre)
            .ok_or(SweepMismatch::MainAccountDebited { pre, post })?;
        if amount_received != self.expected_received() {
            return Err(SweepMismatch::UnexpectedAmountReceived {
                expected: self.expected_received(),
                actual: amount_received,
            }
            .into());
        }

        Ok(SettledSweep {
            amount_received,
            mints: self.mints(),
        })
    }

    /// The mints crediting the deposits of the sweep: each sweepable amount minus the
    /// deposit's share of the fee, rounded up so that the total never exceeds the amount
    /// the minter address receives.
    pub fn mints(&self) -> Vec<CreditedDeposit> {
        let fee_share = self.fee.div_ceil(self.deposit_count() as Lamport);
        self.deposits
            .iter()
            .map(|(deposit_id, deposit)| CreditedDeposit {
                deposit_id: *deposit_id,
                amount_to_mint: deposit
                    .sweepable_amount()
                    .checked_sub(fee_share)
                    .expect("BUG: the minimum deposit amount covers the fee share"),
            })
            .collect()
    }
}

/// A sweep whose outcome matched its plan, with what the minter received and mints for it.
#[derive(Debug, PartialEq, Eq)]
pub struct SettledSweep {
    amount_received: Lamport,
    mints: Vec<CreditedDeposit>,
}

impl SettledSweep {
    pub fn amount_received(&self) -> Lamport {
        self.amount_received
    }

    pub fn into_mints(self) -> Vec<CreditedDeposit> {
        self.mints
    }
}

#[derive(Debug, PartialEq, Eq, Error, From)]
pub enum SweepSettlementError {
    /// The outcome says nothing about the sweep, so fetching it again may succeed.
    #[error("{0}")]
    Unreadable(UnreadableOutcome),
    /// The outcome contradicts the plan of the sweep.
    #[error("{0}")]
    Mismatch(SweepMismatch),
}

#[derive(Debug, PartialEq, Eq, Error)]
pub enum UnreadableOutcome {
    #[error("the sweep transaction could not be decoded")]
    TransactionDecodingFailed,
    #[error("the 'getTransaction' response has no 'meta' field")]
    NoMetaField,
    #[error("the balances in the metadata do not cover all account keys")]
    IncompleteBalances,
}

#[derive(Debug, PartialEq, Eq, Error)]
pub enum SweepMismatch {
    #[error("the sweep transaction failed with {error}")]
    TransactionFailed { error: String },
    #[error("the executed message is not the planned sweep")]
    UnexpectedMessage,
    #[error("the fee of {actual} lamports exceeds the planned fee of {expected} lamports")]
    UnexpectedFee { expected: Lamport, actual: Lamport },
    #[error(
        "the address {address} went from {pre} to {post} lamports instead of decreasing by {expected_decrease}"
    )]
    UnexpectedBalanceChange {
        address: Address,
        pre: Lamport,
        post: Lamport,
        expected_decrease: Lamport,
    },
    #[error(
        "the address {address} is left with {post} lamports, below the rent exemption threshold"
    )]
    NotRentExempt { address: Address, post: Lamport },
    #[error("the main account went from {pre} to {post} lamports")]
    MainAccountDebited { pre: Lamport, post: Lamport },
    #[error("the main account received {actual} lamports instead of the planned {expected}")]
    UnexpectedAmountReceived { expected: Lamport, actual: Lamport },
}

#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum SweepRecoveryError {
    #[error("the message has no transfer to read the destination from")]
    MissingDestination,
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
