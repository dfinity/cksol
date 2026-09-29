use crate::state::QueuedDeposit;
use cksol_types::DepositSolId;
use sol_rpc_types::Lamport;
use solana_signature::Signature;
use std::collections::BTreeMap;

#[cfg(test)]
mod tests;

/// The sweep transactions at one stage of the deposit lifecycle, keyed by signature.
///
/// A deposit belongs to exactly one sweep, so it can also be found by its id.
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
        assert!(
            self.by_signature.insert(signature, sweep).is_none(),
            "Attempted to record sweep {signature} twice"
        );
    }

    pub(super) fn remove(&mut self, signature: &Signature) -> Option<Sweep> {
        self.by_signature.remove(signature)
    }
}

/// The deposits moved to the minter's main account by one sweep transaction.
#[derive(Debug, PartialEq, Eq)]
pub struct Sweep {
    deposits: BTreeMap<DepositSolId, QueuedDeposit>,
}

impl Sweep {
    pub fn new(deposits: BTreeMap<DepositSolId, QueuedDeposit>) -> Self {
        assert!(
            !deposits.is_empty(),
            "Attempted to create a sweep without deposits"
        );
        Self { deposits }
    }

    pub fn deposits(&self) -> &BTreeMap<DepositSolId, QueuedDeposit> {
        &self.deposits
    }

    pub fn swept_amount(&self) -> Lamport {
        self.deposits
            .values()
            .map(|deposit| deposit.sweepable_amount)
            .sum()
    }

    pub fn deposit_count(&self) -> usize {
        self.deposits.len()
    }
}
