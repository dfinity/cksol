use cksol_types::{DepositSolId, DepositSolStatus};
use icrc_ledger_types::icrc1::account::Account;
use sol_rpc_types::Lamport;
use solana_signature::Signature;
use std::collections::BTreeMap;

#[cfg(test)]
mod tests;

/// The deposits swept from their deposit addresses to the minter's main account,
/// keyed by deposit id and grouped by their progress towards a ckSOL mint.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Deposits {
    next_id: DepositSolId,
    queued: BTreeMap<DepositSolId, QueuedDeposit>,
    swept: BTreeMap<DepositSolId, SweptDeposit>,
    in_flight_ids: BTreeMap<Account, DepositSolId>,
}

impl Deposits {
    pub fn next_id(&self) -> DepositSolId {
        self.next_id
    }

    pub fn queued(&self) -> &BTreeMap<DepositSolId, QueuedDeposit> {
        &self.queued
    }

    pub fn swept(&self) -> &BTreeMap<DepositSolId, SweptDeposit> {
        &self.swept
    }

    pub fn in_flight_id(&self, account: &Account) -> Option<DepositSolId> {
        self.in_flight_ids.get(account).copied()
    }

    pub fn status(&self, deposit_id: DepositSolId) -> DepositSolStatus {
        if let Some(deposit) = self.queued.get(&deposit_id) {
            return DepositSolStatus::Queued {
                sweepable_amount: deposit.sweepable_amount,
            };
        }
        if let Some(swept) = self.swept.get(&deposit_id) {
            return DepositSolStatus::Swept {
                signature: swept.signature.into(),
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

    pub(super) fn sweep(&mut self, deposit_id: DepositSolId, signature: &Signature) -> Lamport {
        let deposit = self.queued.remove(&deposit_id).unwrap_or_else(|| {
            panic!("Attempted to sweep unknown or already swept deposit {deposit_id}")
        });
        self.swept.insert(
            deposit_id,
            SweptDeposit {
                deposit,
                signature: *signature,
            },
        );
        deposit.sweepable_amount
    }

    pub(super) fn resubmit_sweep(&mut self, old_signature: &Signature, new_signature: &Signature) {
        for swept in self.swept.values_mut() {
            if &swept.signature == old_signature {
                swept.signature = *new_signature;
            }
        }
    }
}

/// A deposit address queued for a sweep to the minter's main account.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QueuedDeposit {
    pub account: Account,
    pub sweepable_amount: Lamport,
}

/// A queued deposit whose sweep transaction has been submitted but not yet finalized.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SweptDeposit {
    pub deposit: QueuedDeposit,
    pub signature: Signature,
}
