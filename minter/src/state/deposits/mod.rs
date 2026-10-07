use crate::{
    constants::RENT_EXEMPTION_THRESHOLD,
    numeric::LedgerMintIndex,
    state::event::{CreditedDeposit, VersionedMessage},
};
use cksol_types::{DepositSolId, DepositSolStatus};
use icrc_ledger_types::icrc1::account::Account;
use sol_rpc_types::Lamport;
use solana_address::Address;
use solana_signature::Signature;
use std::collections::BTreeMap;

pub use sweeps::{
    SettledSweep, Sweep, SweepMismatch, SweepRecoveryError, SweepSettlementError, Sweeps, Transfer,
    UnreadableOutcome,
};

mod sweeps;
#[cfg(test)]
mod tests;

/// The deposits accepted by `deposit_sol`, grouped by their progress towards a ckSOL mint.
///
/// A deposit is in exactly one stage at a time and only moves forward:
///
/// ```text
/// queued --sendTransaction--> swept --getSignatureStatuses--> finalized --getTransaction--> pending_mints --icrc1_transfer--> minted
///                                |                              |                                |
///                                +--failed or expired--> dropped  +--metadata mismatch--> quarantined <--beyond the deduplication window--+
/// ```
///
/// * `queued`: the deposit address holds a sweepable amount and waits for a sweep transaction.
///   Deposits are keyed by id, so the sweep timer takes them in the order they were queued.
/// * `swept`: a sweep transaction moving the deposits to the main account was submitted.
///   The deposits of one transaction are kept together under its signature, since the
///   transaction is what the finalization timer tracks from here on. A sweep is never
///   resubmitted: when it fails or expires, its deposits are dropped and their accounts
///   released, so that `deposit_sol` can queue a new sweep of the balance still on the
///   deposit address.
/// * `finalized`: `getSignatureStatuses` reported the sweep as finalized without error. The
///   amount received by the main account is still unknown, because Solana may charge a
///   different transaction fee than the sweep was built with.
/// * `pending_mints`: the metadata returned by `getTransaction` was read and the transaction
///   fee of the sweep was shared between its deposits. Each deposit now carries the amount to
///   mint and only the mint on the ledger remains, so the deposits are tracked individually again.
/// * `minted`: the ckSOL mint landed on the ledger and the account is released.
/// * `quarantined`: the metadata contradicts the minter's model of the sweep, so nothing was
///   credited and nothing is minted, or a pending mint could no longer be retried within the
///   deduplication window of the ledger, in which case the sweep was credited and the mint is
///   still owed unless it already landed. The cause records which of the two it is, and the
///   accounts stay rejected by `deposit_sol` until a minter upgrade.
///
/// Every account has at most one deposit in flight, so that `deposit_sol` can report the
/// deposit it is already tracking instead of queueing the same balance twice.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Deposits {
    next_id: DepositSolId,
    queued: BTreeMap<DepositSolId, QueuedDeposit>,
    swept: Sweeps,
    finalized: Sweeps,
    pending_mints: BTreeMap<DepositSolId, PendingMint>,
    minted: BTreeMap<DepositSolId, MintedSweep>,
    dropped: BTreeMap<DepositSolId, SweptDeposit>,
    quarantined: BTreeMap<DepositSolId, QuarantinedDeposit>,
    in_flight_ids: BTreeMap<Account, DepositSolId>,
}

impl Deposits {
    pub fn next_id(&self) -> DepositSolId {
        self.next_id
    }

    pub fn queued(&self) -> &BTreeMap<DepositSolId, QueuedDeposit> {
        &self.queued
    }

    pub fn swept(&self) -> &Sweeps {
        &self.swept
    }

    pub fn finalized(&self) -> &Sweeps {
        &self.finalized
    }

    pub fn pending_mints(&self) -> &BTreeMap<DepositSolId, PendingMint> {
        &self.pending_mints
    }

    pub fn minted(&self) -> &BTreeMap<DepositSolId, MintedSweep> {
        &self.minted
    }

    pub fn dropped(&self) -> &BTreeMap<DepositSolId, SweptDeposit> {
        &self.dropped
    }

    pub fn quarantined(&self) -> &BTreeMap<DepositSolId, QuarantinedDeposit> {
        &self.quarantined
    }

    pub fn in_flight_id(&self, account: &Account) -> Option<DepositSolId> {
        self.in_flight_ids.get(account).copied()
    }

    pub fn status(&self, deposit_id: DepositSolId) -> DepositSolStatus {
        if let Some(deposit) = self.queued.get(&deposit_id) {
            return DepositSolStatus::Queued {
                sweepable_amount: deposit.sweepable_amount(),
            };
        }
        if let Some((signature, _)) = self.swept.deposit(deposit_id) {
            return DepositSolStatus::Swept {
                signature: (*signature).into(),
            };
        }
        if let Some((signature, _)) = self.finalized.deposit(deposit_id) {
            return DepositSolStatus::Finalized {
                signature: (*signature).into(),
            };
        }
        if let Some(pending) = self.pending_mints.get(&deposit_id) {
            return DepositSolStatus::Finalized {
                signature: pending.sweep_signature().into(),
            };
        }
        if let Some(minted) = self.minted.get(&deposit_id) {
            return DepositSolStatus::Minted {
                block_index: *minted.mint_block_index.get(),
                minted_amount: minted.minted_amount,
            };
        }
        if let Some(dropped) = self.dropped.get(&deposit_id) {
            return DepositSolStatus::Dropped {
                signature: dropped.signature.into(),
            };
        }
        if let Some(quarantined) = self.quarantined.get(&deposit_id) {
            return DepositSolStatus::Quarantined {
                signature: quarantined.deposit.signature.into(),
            };
        }
        DepositSolStatus::NotFound
    }

    pub(super) fn queue(&mut self, deposit_id: DepositSolId, deposit: QueuedDeposit) {
        assert_eq!(
            deposit_id, self.next_id,
            "Attempted to queue deposit {deposit_id} out of sequence, expected {}",
            self.next_id
        );
        assert!(
            self.in_flight_ids
                .insert(deposit.account, deposit_id)
                .is_none(),
            "Attempted to queue a deposit for account {:?} that already has one in flight",
            deposit.account
        );
        self.queued.insert(deposit_id, deposit);
        self.next_id += 1;
    }

    /// Moves the given queued deposits to the sweep submitted with the given message and
    /// signature, checked to sweep them to the given minter address, and returns the
    /// amount the sweep transfers to the main account.
    pub(super) fn sweep(
        &mut self,
        deposit_ids: &[DepositSolId],
        minter_address: Address,
        message: &VersionedMessage,
        signature: &Signature,
    ) -> Lamport {
        assert!(
            !deposit_ids.is_empty(),
            "Attempted to sweep no deposits with transaction {signature}"
        );
        let deposits: Vec<_> = deposit_ids
            .iter()
            .map(|deposit_id| {
                let deposit = self.queued.remove(deposit_id).unwrap_or_else(|| {
                    panic!("Attempted to sweep unknown or already swept deposit {deposit_id}")
                });
                (*deposit_id, deposit)
            })
            .collect();
        let sweep = Sweep::recover(deposits, minter_address, message)
            .unwrap_or_else(|e| panic!("Attempted to sweep with transaction {signature}: {e}"));
        let expected_received = sweep.expected_received();
        self.swept.insert(*signature, sweep);
        expected_received
    }

    /// Drops every deposit of the given swept sweep and releases their accounts.
    pub(super) fn drop_swept(&mut self, signature: &Signature) {
        let sweep = self
            .swept
            .remove(signature)
            .unwrap_or_else(|| panic!("Attempted to drop sweep {signature} that is not swept"));
        for (deposit_id, deposit) in sweep.deposits() {
            self.release_in_flight(*deposit_id, &deposit.account);
            self.dropped.insert(
                *deposit_id,
                SweptDeposit {
                    deposit: *deposit,
                    signature: *signature,
                },
            );
        }
    }

    /// Moves every deposit of the given finalized sweep to the quarantine, keeping their
    /// accounts in flight.
    pub(super) fn quarantine_sweep(&mut self, signature: &Signature) {
        let sweep = self.finalized.remove(signature).unwrap_or_else(|| {
            panic!("Attempted to quarantine sweep {signature} that is not finalized")
        });
        self.quarantined
            .extend(sweep.deposits().iter().map(|(deposit_id, deposit)| {
                (
                    *deposit_id,
                    QuarantinedDeposit {
                        deposit: SweptDeposit {
                            deposit: *deposit,
                            signature: *signature,
                        },
                        cause: QuarantineCause::SweepUnreadable,
                    },
                )
            }));
    }

    fn release_in_flight(&mut self, deposit_id: DepositSolId, account: &Account) {
        assert_eq!(
            self.in_flight_ids.remove(account),
            Some(deposit_id),
            "BUG: deposit {deposit_id} is not the in-flight deposit of account {account:?}"
        );
    }

    pub(super) fn finalize_swept(&mut self, signature: &Signature) {
        let sweep = self
            .swept
            .remove(signature)
            .unwrap_or_else(|| panic!("Attempted to finalize sweep {signature} that is not swept"));
        self.finalized.insert(*signature, sweep);
    }

    /// Moves every deposit of the given finalized sweep to the pending mints, each with
    /// the amount its mint carries.
    pub(super) fn credit_sweep(
        &mut self,
        signature: &Signature,
        mints: &[CreditedDeposit],
        timestamp: u64,
    ) {
        let sweep = self.finalized.remove(signature).unwrap_or_else(|| {
            panic!("Attempted to credit sweep {signature} that is not finalized")
        });
        assert_eq!(
            mints.len(),
            sweep.deposit_count(),
            "Attempted to credit sweep {signature} with {} mints for {} deposits",
            mints.len(),
            sweep.deposit_count()
        );
        for mint in mints {
            let deposit = sweep.deposits().get(&mint.deposit_id).unwrap_or_else(|| {
                panic!(
                    "Attempted to credit deposit {} that is not part of sweep {signature}",
                    mint.deposit_id
                )
            });
            assert!(
                mint.amount_to_mint <= deposit.sweepable_amount(),
                "Attempted to mint {} lamports for deposit {} beyond its sweepable amount of {} lamports",
                mint.amount_to_mint,
                mint.deposit_id,
                deposit.sweepable_amount()
            );
            let pending = PendingMint {
                deposit: SweptDeposit {
                    deposit: *deposit,
                    signature: *signature,
                },
                amount_to_mint: mint.amount_to_mint,
                created_at_time: timestamp,
            };
            assert!(
                self.pending_mints
                    .insert(mint.deposit_id, pending)
                    .is_none(),
                "Attempted to credit deposit {} twice in sweep {signature}",
                mint.deposit_id
            );
        }
    }

    pub(super) fn mint(&mut self, deposit_id: DepositSolId, mint_block_index: LedgerMintIndex) {
        let pending = self.pending_mints.remove(&deposit_id).unwrap_or_else(|| {
            panic!("Attempted to mint deposit {deposit_id} that has no pending mint")
        });
        self.release_in_flight(deposit_id, &pending.account());
        self.minted.insert(
            deposit_id,
            MintedSweep {
                deposit: pending.deposit,
                minted_amount: pending.amount_to_mint,
                mint_block_index,
            },
        );
    }

    pub(super) fn quarantine_pending_mint(&mut self, deposit_id: DepositSolId) {
        let pending = self.pending_mints.remove(&deposit_id).unwrap_or_else(|| {
            panic!("Attempted to quarantine deposit {deposit_id} that has no pending mint")
        });
        self.quarantined.insert(
            deposit_id,
            QuarantinedDeposit {
                deposit: pending.deposit,
                cause: QuarantineCause::MintUnresolved {
                    amount_to_mint: pending.amount_to_mint,
                    created_at_time: pending.created_at_time,
                },
            },
        );
    }
}

/// A deposit address queued for a sweep to the minter's main account.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QueuedDeposit {
    /// The account credited with ckSOL once the sweep is finalized.
    pub account: Account,
    /// The deposit address derived from the account, controlled by the minter.
    pub address: Address,
    /// The balance of the deposit address when the deposit was queued.
    pub balance: DepositBalance,
}

impl QueuedDeposit {
    pub fn sweepable_amount(&self) -> Lamport {
        self.balance.sweepable_amount()
    }
}

impl From<DepositBalance> for Lamport {
    fn from(balance: DepositBalance) -> Self {
        balance.0
    }
}

/// A deposit address balance that stays rent-exempt once its sweepable amount is transferred.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct DepositBalance(Lamport);

impl DepositBalance {
    /// The balance, if it covers the rent exemption threshold.
    pub fn new(balance: Lamport) -> Option<Self> {
        (balance >= RENT_EXEMPTION_THRESHOLD).then_some(Self(balance))
    }

    /// The balance minus the rent exemption threshold left on the deposit address.
    pub fn sweepable_amount(self) -> Lamport {
        self.0
            .checked_sub(RENT_EXEMPTION_THRESHOLD)
            .expect("BUG: a deposit balance covers the rent exemption threshold")
    }
}

/// A deposit together with the sweep transaction that was submitted to move it to the main account.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SweptDeposit {
    pub deposit: QueuedDeposit,
    pub signature: Signature,
}

/// A swept deposit whose sweep reached the minter's main account and whose ckSOL
/// mint has not been sent to the ledger yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PendingMint {
    pub deposit: SweptDeposit,
    /// The sweepable amount minus the deposit's share of the transaction fee of the sweep.
    pub amount_to_mint: Lamport,
    /// The timestamp of the `CreditedSweep` event that enqueued this pending mint.
    ///
    /// Every retry of the mint sends this same `created_at_time` to the ckSOL
    /// ledger, so the ledger deduplicates retries after an unknown outcome.
    pub created_at_time: u64,
}

impl PendingMint {
    pub fn account(&self) -> Account {
        self.deposit.deposit.account
    }

    pub fn sweep_signature(&self) -> Signature {
        self.deposit.signature
    }
}

/// A swept deposit the minter stopped processing, together with what resolving it takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QuarantinedDeposit {
    pub deposit: SweptDeposit,
    pub cause: QuarantineCause,
}

impl QuarantinedDeposit {
    pub fn account(&self) -> Account {
        self.deposit.deposit.account
    }

    pub fn sweep_signature(&self) -> Signature {
        self.deposit.signature
    }

    /// The amount the sweep planned to move, before the deposit's share of its fee.
    pub fn planned_amount(&self) -> Lamport {
        self.deposit.deposit.sweepable_amount()
    }
}

/// Why a swept deposit was quarantined, and therefore what a manual resolution has
/// to establish before crediting it by hand.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuarantineCause {
    /// The finalized sweep contradicted the plan it was submitted with, so the amount
    /// received could not be determined: nothing was credited and nothing was minted.
    SweepUnreadable,
    /// The sweep was credited and this mint is owed, but it fell out of the
    /// deduplication window of the ledger before it was confirmed, so it may already
    /// have landed. The ledger has to be searched for a mint carrying this deposit in
    /// its memo before minting again.
    MintUnresolved {
        /// The amount the pending mint was going to mint.
        amount_to_mint: Lamport,
        /// The `created_at_time` the pending mint was sent with.
        created_at_time: u64,
    },
}

/// A swept deposit whose ckSOL mint landed on the ledger.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MintedSweep {
    pub deposit: SweptDeposit,
    /// The sweepable amount minus the deposit's share of the transaction fee of the sweep.
    pub minted_amount: Lamport,
    pub mint_block_index: LedgerMintIndex,
}
