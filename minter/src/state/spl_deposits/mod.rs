use crate::state::{
    SchnorrPublicKey, SupportedSplToken,
    event::{Signer, VersionedMessage},
};
use cksol_types::DepositSplId;
use icrc_ledger_types::icrc1::account::Account;
use sol_rpc_types::Lamport;
use solana_address::Address;
use solana_signature::Signature;
use std::collections::BTreeMap;

#[cfg(test)]
mod tests;

mod sweeps;
pub use sweeps::{
    SPL_SWEEP_TRANSACTION_FEE, SplSweep, SplSweepRecoveryError, SplSweeps, SubmittedSplSweep,
};

/// SPL deposits grouped by their progress towards a token mint.
///
/// A submitted sweep is `swept` until its outcome is observed. An observed success is
/// `finalized` until it is credited. An observed failure is `failed` until its actual cost
/// is validated. Each account and mint pair can have at most one deposit in flight.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SplDeposits {
    next_id: DepositSplId,
    queued: BTreeMap<DepositSplId, QueuedSplDeposit>,
    swept: SplSweeps,
    finalized: SplSweeps,
    failed: SplSweeps,
    dropped: BTreeMap<DepositSplId, SweptSplDeposit>,
    in_flight_ids: BTreeMap<(Account, Address), DepositSplId>,
    latest_ids: BTreeMap<(Account, Address), DepositSplId>,
    queued_at_by_id: BTreeMap<DepositSplId, u64>,
}

impl SplDeposits {
    pub fn next_id(&self) -> DepositSplId {
        self.next_id
    }

    pub fn queued(&self) -> &BTreeMap<DepositSplId, QueuedSplDeposit> {
        &self.queued
    }

    pub fn swept(&self) -> &SplSweeps {
        &self.swept
    }

    pub fn finalized(&self) -> &SplSweeps {
        &self.finalized
    }

    /// Sweeps observed as failed on-chain. Each keeps its reservation until its cost is validated.
    pub fn failed(&self) -> &SplSweeps {
        &self.failed
    }

    pub fn dropped(&self) -> &BTreeMap<DepositSplId, SweptSplDeposit> {
        &self.dropped
    }

    pub fn in_flight_id(&self, account: &Account, mint: &Address) -> Option<DepositSplId> {
        self.in_flight_ids.get(&(*account, *mint)).copied()
    }

    /// The id of the latest deposit queued for the account and mint pair, whatever stage it
    /// reached since.
    pub fn latest_id(&self, account: &Account, mint: &Address) -> Option<DepositSplId> {
        self.latest_ids.get(&(*account, *mint)).copied()
    }

    pub fn oldest_in_flight_queued_at(&self) -> Option<u64> {
        self.queued_at_by_id.values().min().copied()
    }

    pub(super) fn queue(
        &mut self,
        deposit_id: DepositSplId,
        deposit: QueuedSplDeposit,
        queued_at: u64,
    ) {
        assert_eq!(
            deposit_id, self.next_id,
            "Attempted to queue SPL deposit {deposit_id} out of sequence, expected {}",
            self.next_id
        );
        let key = (deposit.account, deposit.mint);
        assert!(
            !self.in_flight_ids.contains_key(&key),
            "Attempted to queue an SPL deposit for account {:?} and mint {} that already has one in flight",
            deposit.account,
            deposit.mint
        );
        assert!(
            deposit.fee < deposit.balance,
            "Attempted to queue SPL deposit {deposit_id} with a fee at or above its balance"
        );
        let next_id = self
            .next_id
            .checked_add(1)
            .expect("SPL deposit ID overflow");
        self.in_flight_ids.insert(key, deposit_id);
        self.latest_ids.insert(key, deposit_id);
        self.queued_at_by_id.insert(deposit_id, queued_at);
        self.queued.insert(deposit_id, deposit);
        self.next_id = next_id;
    }

    /// Moves the queued deposit to its submitted sweep. Its account and mint stay in flight
    /// until the sweep and the ledger mint are resolved.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn sweep(
        &mut self,
        deposit_id: DepositSplId,
        message: &VersionedMessage,
        signers: &[Signer],
        tokens: &BTreeMap<Address, SupportedSplToken>,
        master_key: &SchnorrPublicKey,
        signature: &Signature,
        reserved_lamports: Lamport,
    ) {
        let deposit = self.queued.get(&deposit_id).unwrap_or_else(|| {
            panic!("Attempted to sweep unknown or already swept SPL deposit {deposit_id}")
        });
        let token = tokens
            .get(&deposit.mint)
            .unwrap_or_else(|| panic!("BUG: SPL sweep mint {} must be registered", deposit.mint));
        let sweep = SplSweep::recover(
            deposit_id,
            deposit.clone(),
            token,
            master_key,
            message,
            signers,
        )
        .unwrap_or_else(|error| panic!("Attempted to sweep with transaction {signature}: {error}"));
        self.queued.remove(&deposit_id);
        self.swept.insert(
            *signature,
            SubmittedSplSweep {
                sweep,
                reserved_lamports,
            },
        );
    }

    /// Records an observed success. The reservation stays until the credit step settles it.
    pub(super) fn finalize_swept(&mut self, signature: &Signature) {
        let sweep = self.swept.remove(signature).unwrap_or_else(|| {
            panic!("Attempted to finalize SPL sweep {signature} that is not swept")
        });
        self.finalized.insert(*signature, sweep);
    }

    /// Records an observed failure. Its cost is unknown, so the reservation and the pair stay.
    pub(super) fn fail_swept(&mut self, signature: &Signature) {
        let sweep = self
            .swept
            .remove(signature)
            .unwrap_or_else(|| panic!("Attempted to fail SPL sweep {signature} that is not swept"));
        self.failed.insert(*signature, sweep);
    }

    /// Drops an expired sweep and releases its account and mint pair. Returns the sweep
    /// for its reservation.
    pub(super) fn drop_swept(&mut self, signature: &Signature) -> SubmittedSplSweep {
        let sweep = self
            .swept
            .remove(signature)
            .unwrap_or_else(|| panic!("Attempted to drop SPL sweep {signature} that is not swept"));
        self.drop_deposit(&sweep.sweep, signature);
        sweep
    }

    fn drop_deposit(&mut self, sweep: &SplSweep, signature: &Signature) {
        let deposit_id = sweep.deposit_id();
        self.release_in_flight(deposit_id, sweep.deposit());
        self.dropped.insert(
            deposit_id,
            SweptSplDeposit {
                deposit: sweep.deposit().clone(),
                signature: *signature,
            },
        );
    }

    fn release_in_flight(&mut self, deposit_id: DepositSplId, deposit: &QueuedSplDeposit) {
        assert_eq!(
            self.in_flight_ids.remove(&(deposit.account, deposit.mint)),
            Some(deposit_id),
            "BUG: SPL deposit {deposit_id} is not in flight"
        );
        self.queued_at_by_id.remove(&deposit_id);
    }
}

/// A finalized token-account balance accepted for a future sweep.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueuedSplDeposit {
    pub account: Account,
    pub mint: Address,
    pub address: Address,
    /// The token-account balance when the deposit was queued, in the mint's smallest units.
    pub balance: u64,
    /// The fee recorded at acceptance, deducted from the ledger mint in the mint's smallest units.
    pub fee: u64,
}

impl QueuedSplDeposit {
    pub fn amount_to_mint(&self) -> u64 {
        self.balance
            .checked_sub(self.fee)
            .expect("BUG: a queued SPL deposit fee must not exceed its balance")
    }
}

/// A deposit together with the sweep transaction submitted to move its tokens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SweptSplDeposit {
    pub deposit: QueuedSplDeposit,
    pub signature: Signature,
}
