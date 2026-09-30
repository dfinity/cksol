use crate::constants::RENT_EXEMPTION_THRESHOLD;
use cksol_types::{DepositSolId, DepositSolStatus};
use icrc_ledger_types::icrc1::account::Account;
use sol_rpc_types::Lamport;
use solana_address::Address;
use solana_signature::Signature;
use std::collections::BTreeMap;

pub use sweeps::{Sweep, Sweeps, Transfer};

mod sweeps;
#[cfg(test)]
mod tests;

/// The deposits accepted by `deposit_sol`, grouped by their progress towards a ckSOL mint.
///
/// A deposit is in exactly one stage at a time and only moves forward:
///
/// ```text
/// queued --sendTransaction--> swept
/// ```
///
/// * `queued`: the deposit address holds a sweepable amount and waits for a sweep transaction.
///   Deposits are keyed by id, so the sweep timer takes them in the order they were queued.
/// * `swept`: a sweep transaction moving the deposits to the main account was submitted.
///   The deposits of one transaction are kept together under its signature, since the
///   transaction is what the finalization timer tracks from here on. The signature changes
///   whenever the sweep expires and is resubmitted with a fresh blockhash.
///
/// Every account has at most one deposit in flight, so that `deposit_sol` can report the
/// deposit it is already tracking instead of queueing the same balance twice.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Deposits {
    next_id: DepositSolId,
    queued: BTreeMap<DepositSolId, QueuedDeposit>,
    swept: Sweeps,
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

    /// Moves the given queued deposits to the sweep with the given signature and
    /// returns the amount the sweep transfers to the main account.
    pub(super) fn sweep(
        &mut self,
        deposit_ids: &[DepositSolId],
        minter_address: Address,
        signature: &Signature,
    ) -> Lamport {
        assert!(
            !deposit_ids.is_empty(),
            "Attempted to sweep no deposits with transaction {signature}"
        );
        let sweep = Sweep::plan(
            deposit_ids.iter().map(|deposit_id| {
                let deposit = self.queued.remove(deposit_id).unwrap_or_else(|| {
                    panic!("Attempted to sweep unknown or already swept deposit {deposit_id}")
                });
                (*deposit_id, deposit)
            }),
            minter_address,
        );
        let swept_amount = sweep.swept_amount();
        self.swept.insert(*signature, sweep);
        swept_amount
    }

    pub(super) fn resubmit_sweep(&mut self, old_signature: &Signature, new_signature: &Signature) {
        if let Some(sweep) = self.swept.remove(old_signature) {
            self.swept.insert(*new_signature, sweep);
        }
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

/// A deposit address balance that stays rent-exempt once its sweepable amount is transferred.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct DepositBalance(Lamport);

impl DepositBalance {
    /// The balance, if it covers the rent exemption threshold.
    pub fn new(balance: Lamport) -> Option<Self> {
        (balance >= RENT_EXEMPTION_THRESHOLD).then_some(Self(balance))
    }

    pub fn get(self) -> Lamport {
        self.0
    }

    /// The balance minus the rent exemption threshold left on the deposit address.
    pub fn sweepable_amount(self) -> Lamport {
        self.0 - RENT_EXEMPTION_THRESHOLD
    }
}
