use cksol_types::DepositSplId;
use icrc_ledger_types::icrc1::account::Account;
use solana_address::Address;
use std::collections::BTreeMap;

#[cfg(test)]
mod tests;

/// SPL deposits grouped by their progress towards a token mint.
///
/// Each account and mint pair can have at most one deposit in flight.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SplDeposits {
    next_id: DepositSplId,
    queued: BTreeMap<DepositSplId, QueuedSplDeposit>,
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
