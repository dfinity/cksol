use crate::{
    constants::RENT_EXEMPTION_THRESHOLD,
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
/// queued --sendTransaction--> swept --getSignatureStatuses--> finalized --getTransaction--> pending_mints
///                                |                              |
///                                +--failed or expired--> dropped  +--metadata mismatch--> quarantined
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
/// * `quarantined`: the metadata contradicts the minter's model of the sweep, so nothing is
///   minted and the accounts stay rejected by `deposit_sol` until manual intervention.
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
    dropped: BTreeMap<DepositSolId, SweptDeposit>,
    quarantined: BTreeMap<DepositSolId, SweptDeposit>,
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

    pub fn dropped(&self) -> &BTreeMap<DepositSolId, SweptDeposit> {
        &self.dropped
    }

    pub fn quarantined(&self) -> &BTreeMap<DepositSolId, SweptDeposit> {
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
                signature: pending.deposit.signature.into(),
            };
        }
        if let Some(dropped) = self.dropped.get(&deposit_id) {
            return DepositSolStatus::Dropped {
                signature: dropped.signature.into(),
            };
        }
        if let Some(quarantined) = self.quarantined.get(&deposit_id) {
            return DepositSolStatus::Quarantined {
                signature: quarantined.signature.into(),
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
    /// signature and returns the amount the sweep transfers to the main account.
    pub(super) fn sweep(
        &mut self,
        deposit_ids: &[DepositSolId],
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
        let sweep = Sweep::recover(deposits, message)
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
        for (deposit_id, dropped) in swept_deposits(&sweep, signature) {
            self.release_in_flight(deposit_id, &dropped.deposit.account);
            self.dropped.insert(deposit_id, dropped);
        }
    }

    /// Moves every deposit of the given finalized sweep to the quarantine, keeping their
    /// accounts in flight.
    pub(super) fn quarantine_sweep(&mut self, signature: &Signature) {
        let sweep = self.finalized.remove(signature).unwrap_or_else(|| {
            panic!("Attempted to quarantine sweep {signature} that is not finalized")
        });
        self.quarantined.extend(swept_deposits(&sweep, signature));
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
    pub(super) fn credit_sweep(&mut self, signature: &Signature, mints: &[CreditedDeposit]) {
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
}

fn swept_deposits<'a>(
    sweep: &'a Sweep,
    signature: &'a Signature,
) -> impl Iterator<Item = (DepositSolId, SweptDeposit)> + 'a {
    sweep.deposits().iter().map(|(deposit_id, deposit)| {
        (
            *deposit_id,
            SweptDeposit {
                deposit: *deposit,
                signature: *signature,
            },
        )
    })
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
}
