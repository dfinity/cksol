use crate::numeric::LedgerMintIndex;
use crate::state::{
    SchnorrPublicKey, SupportedSplToken,
    event::{Signer, VersionedMessage},
};
use cksol_types::{DepositSplId, DepositSplStatus};
use icrc_ledger_types::icrc1::account::Account;
use sol_rpc_types::Lamport;
use solana_address::Address;
use solana_signature::Signature;
use std::collections::BTreeMap;

#[cfg(test)]
mod tests;

mod sweeps;
pub use sweeps::{
    SPL_SWEEP_TRANSACTION_FEE, SettledSplSweep, SplSweep, SplSweepRecoveryError, SplSweeps,
    SubmittedSplSweep,
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
    pending_mints: BTreeMap<DepositSplId, PendingSplMint>,
    minted: BTreeMap<DepositSplId, MintedSplSweep>,
    dropped: BTreeMap<DepositSplId, SweptSplDeposit>,
    quarantined: BTreeMap<DepositSplId, QuarantinedSplDeposit>,
    in_flight_ids: BTreeMap<(Account, Address), DepositSplId>,
    latest_ids: BTreeMap<(Account, Address), DepositSplId>,
    queued_at_by_id: BTreeMap<DepositSplId, u64>,
    collected_deposit_fees: BTreeMap<Address, u128>,
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

    pub fn pending_mints(&self) -> &BTreeMap<DepositSplId, PendingSplMint> {
        &self.pending_mints
    }

    pub fn quarantined(&self) -> &BTreeMap<DepositSplId, QuarantinedSplDeposit> {
        &self.quarantined
    }

    pub fn minted(&self) -> &BTreeMap<DepositSplId, MintedSplSweep> {
        &self.minted
    }

    /// The token fees retained in the treasury of the mint, in its smallest units.
    pub fn collected_deposit_fees(&self, mint: &Address) -> u128 {
        self.collected_deposit_fees.get(mint).copied().unwrap_or(0)
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

    /// The deposit of the account and mint pair that is in flight, with whether it is still
    /// processed or quarantined.
    pub fn in_flight(&self, account: &Account, mint: &Address) -> Option<InFlightSplDeposit> {
        let deposit_id = self.in_flight_id(account, mint)?;
        Some(if self.quarantined.contains_key(&deposit_id) {
            InFlightSplDeposit::Quarantined(deposit_id)
        } else {
            InFlightSplDeposit::Processing(deposit_id)
        })
    }

    pub fn status(&self, deposit_id: DepositSplId) -> DepositSplStatus {
        if let Some(deposit) = self.queued.get(&deposit_id) {
            return DepositSplStatus::Queued {
                balance: deposit.balance,
                fee: deposit.fee,
            };
        }
        // A sweep observed as failed is not settled yet, so its deposit is still swept.
        if let Some((signature, _)) = self
            .swept
            .deposit(deposit_id)
            .or_else(|| self.failed.deposit(deposit_id))
        {
            return DepositSplStatus::Swept {
                signature: (*signature).into(),
            };
        }
        if let Some((signature, _)) = self.finalized.deposit(deposit_id) {
            return DepositSplStatus::Finalized {
                signature: (*signature).into(),
            };
        }
        if let Some(pending) = self.pending_mints.get(&deposit_id) {
            return DepositSplStatus::Finalized {
                signature: pending.sweep_signature().into(),
            };
        }
        if let Some(minted) = self.minted.get(&deposit_id) {
            return DepositSplStatus::Minted {
                block_index: *minted.mint_block_index.get(),
                minted_amount: minted.minted_amount,
            };
        }
        if let Some(dropped) = self.dropped.get(&deposit_id) {
            return DepositSplStatus::Dropped {
                signature: dropped.signature.into(),
            };
        }
        if let Some(quarantined) = self.quarantined.get(&deposit_id) {
            return DepositSplStatus::Quarantined {
                signature: quarantined.sweep_signature().into(),
            };
        }
        DepositSplStatus::NotFound
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

    /// Moves the deposit of a finalized sweep to its pending token mint and counts its token
    /// fee, which stays in the treasury. Returns the settled sweep for its reservation.
    pub(super) fn credit_sweep(
        &mut self,
        signature: &Signature,
        deposit_id: DepositSplId,
        amount_to_mint: u64,
        timestamp: u64,
    ) -> SubmittedSplSweep {
        let submitted = self.finalized.get(signature).unwrap_or_else(|| {
            panic!("Attempted to credit SPL sweep {signature} that is not finalized")
        });
        let deposit = submitted.sweep.deposit();
        assert_eq!(
            deposit_id,
            submitted.sweep.deposit_id(),
            "Attempted to credit SPL deposit {deposit_id} that is not part of sweep {signature}"
        );
        assert_eq!(
            amount_to_mint,
            deposit.amount_to_mint(),
            "Attempted to credit SPL deposit {deposit_id} with an amount different from its transferred balance minus the fee"
        );
        assert!(
            !self.pending_mints.contains_key(&deposit_id),
            "Attempted to credit SPL deposit {deposit_id} twice in sweep {signature}"
        );
        let collected = self
            .collected_deposit_fees(&deposit.mint)
            .checked_add(u128::from(deposit.fee))
            .expect("BUG: collected SPL deposit fees overflow");
        let submitted = self
            .finalized
            .remove(signature)
            .expect("BUG: the finalized sweep was read above");
        let deposit = submitted.sweep.deposit().clone();
        self.collected_deposit_fees.insert(deposit.mint, collected);
        self.pending_mints.insert(
            deposit_id,
            PendingSplMint {
                deposit: SweptSplDeposit {
                    deposit,
                    signature: *signature,
                },
                amount_to_mint,
                created_at_time: timestamp,
            },
        );
        submitted
    }

    /// Quarantines the deposit of a sweep whose observed outcome contradicts its plan,
    /// keeping its account and mint in flight and its SOL reserved.
    pub(super) fn quarantine_sweep(&mut self, signature: &Signature) {
        let submitted = self
            .finalized
            .remove(signature)
            .or_else(|| self.failed.remove(signature))
            .unwrap_or_else(|| {
                panic!("Attempted to quarantine SPL sweep {signature} without an observed outcome")
            });
        let deposit_id = submitted.sweep.deposit_id();
        self.queued_at_by_id.remove(&deposit_id);
        self.quarantined.insert(
            deposit_id,
            QuarantinedSplDeposit {
                deposit: SweptSplDeposit {
                    deposit: submitted.sweep.deposit().clone(),
                    signature: *signature,
                },
                cause: SplQuarantineCause::SweepUnreadable {
                    reserved_lamports: submitted.reserved_lamports,
                },
            },
        );
    }

    pub(super) fn mint(&mut self, deposit_id: DepositSplId, mint_block_index: LedgerMintIndex) {
        let pending = self.pending_mints.remove(&deposit_id).unwrap_or_else(|| {
            panic!("Attempted to mint SPL deposit {deposit_id} that has no pending mint")
        });
        self.release_in_flight(deposit_id, &pending.deposit.deposit);
        self.minted.insert(
            deposit_id,
            MintedSplSweep {
                deposit: pending.deposit,
                minted_amount: pending.amount_to_mint,
                mint_block_index,
            },
        );
    }

    /// Keeps the pair in flight: a new deposit could otherwise hide the unresolved mint.
    pub(super) fn quarantine_pending_mint(&mut self, deposit_id: DepositSplId) {
        let pending = self.pending_mints.remove(&deposit_id).unwrap_or_else(|| {
            panic!("Attempted to quarantine SPL deposit {deposit_id} that has no pending mint")
        });
        self.queued_at_by_id.remove(&deposit_id);
        self.quarantined.insert(
            deposit_id,
            QuarantinedSplDeposit {
                deposit: pending.deposit,
                cause: SplQuarantineCause::MintUnresolved {
                    amount_to_mint: pending.amount_to_mint,
                    created_at_time: pending.created_at_time,
                },
            },
        );
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

    /// Drops a sweep whose failure was observed before and releases its account and mint
    /// pair. Returns the sweep for its reservation.
    pub(super) fn drop_failed(&mut self, signature: &Signature) -> SubmittedSplSweep {
        let sweep = self.failed.remove(signature).unwrap_or_else(|| {
            panic!("Attempted to settle SPL sweep {signature} whose failure was not recorded")
        });
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

/// The deposit that reserves its account and mint pair: no other deposit can be queued for
/// the pair until it is minted or dropped, and a quarantined one keeps the pair reserved
/// until a minter upgrade resolves it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InFlightSplDeposit {
    /// The deposit is queued, swept, finalized or waiting for its mint.
    Processing(DepositSplId),
    /// The deposit is quarantined and keeps its pair rejected until a minter upgrade.
    Quarantined(DepositSplId),
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

/// A swept SPL deposit waiting for its token ledger mint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingSplMint {
    pub deposit: SweptSplDeposit,
    pub amount_to_mint: u64,
    /// The credit event's timestamp, reused by ledger retries for deduplication.
    pub created_at_time: u64,
}

impl PendingSplMint {
    pub fn account(&self) -> Account {
        self.deposit.deposit.account
    }

    pub fn sweep_signature(&self) -> Signature {
        self.deposit.signature
    }
}

/// A swept SPL deposit whose token ledger mint completed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MintedSplSweep {
    pub deposit: SweptSplDeposit,
    pub minted_amount: u64,
    pub mint_block_index: LedgerMintIndex,
}

/// A swept SPL deposit the minter stopped processing, together with what resolving it takes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuarantinedSplDeposit {
    pub deposit: SweptSplDeposit,
    pub cause: SplQuarantineCause,
}

impl QuarantinedSplDeposit {
    pub fn account(&self) -> Account {
        self.deposit.deposit.account
    }

    pub fn sweep_signature(&self) -> Signature {
        self.deposit.signature
    }
}

/// Why a swept SPL deposit was quarantined, and therefore what a manual resolution has to
/// establish before crediting it by hand.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplQuarantineCause {
    /// The observed outcome of the sweep contradicted its plan: nothing was credited and
    /// nothing was minted. The SOL reserved for the sweep stays withheld.
    SweepUnreadable {
        /// The SOL still withheld from both spendable balances.
        reserved_lamports: Lamport,
    },
    /// The sweep was credited and this mint is owed, but it may already have landed. The
    /// token ledger has to be searched for a mint carrying the sweep signature in its memo
    /// before minting again.
    MintUnresolved {
        /// The amount the pending mint was going to mint.
        amount_to_mint: u64,
        /// The `created_at_time` the pending mint was sent with.
        created_at_time: u64,
    },
}
