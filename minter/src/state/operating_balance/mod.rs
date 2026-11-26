use super::{InvalidStateError, State};
use cksol_types_internal::OperatingRefillConfig;

#[cfg(test)]
mod tests;

/// Burn terms that survive retries and upgrades. Quarantine retains the entire intent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PendingOperatingRefill {
    pub refill_id: u64,
    pub amount: u64,
    pub created_at_time: u64,
    pub quarantined: bool,
}

/// Unbacked operating SOL, independent of the observed main-address SOL balance.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OperatingBalance {
    balance: u64,
    config: OperatingRefillConfig,
    next_refill_id: u64,
    pending: Option<PendingOperatingRefill>,
    completed_refills: u64,
    failed_refills: u64,
}

impl State {
    pub fn operating_balance(&self) -> &OperatingBalance {
        &self.operating_balance
    }
}

impl OperatingBalance {
    pub fn balance(&self) -> u64 {
        self.balance
    }

    pub fn config(&self) -> OperatingRefillConfig {
        self.config
    }

    pub fn pending(&self) -> Option<PendingOperatingRefill> {
        self.pending
    }

    pub fn next_refill_id(&self) -> u64 {
        self.next_refill_id
    }

    pub fn completed_refills(&self) -> u64 {
        self.completed_refills
    }

    pub fn failed_refills(&self) -> u64 {
        self.failed_refills
    }

    /// Amount needed to reach the target; zero when a new refill is not due.
    pub fn refill_amount(&self) -> u64 {
        if !self.config.enabled
            || self.pending.is_some()
            || self.balance >= self.config.low_water_mark
        {
            return 0;
        }
        self.config.target.saturating_sub(self.balance)
    }

    pub(super) fn set_config(&mut self, config: OperatingRefillConfig) {
        self.config = config;
    }

    pub(super) fn validate(&self) -> Result<(), InvalidStateError> {
        if self.config.enabled
            && (self.config.low_water_mark == 0 || self.config.low_water_mark >= self.config.target)
        {
            return Err(InvalidStateError::InvalidOperatingRefillConfig);
        }
        Ok(())
    }

    pub(super) fn request_refill(&mut self, refill_id: u64, amount: u64, created_at_time: u64) {
        assert_eq!(refill_id, self.next_refill_id, "BUG: unexpected refill ID");
        assert!(
            amount > 0 && amount <= self.refill_amount(),
            "BUG: invalid refill amount"
        );
        let next_refill_id = refill_id.checked_add(1).expect("BUG: refill ID overflow");
        self.pending = Some(PendingOperatingRefill {
            refill_id,
            amount,
            created_at_time,
            quarantined: false,
        });
        self.next_refill_id = next_refill_id;
    }

    fn pending_refill(&self, refill_id: u64) -> PendingOperatingRefill {
        let pending = self.pending.expect("BUG: no pending refill");
        assert_eq!(pending.refill_id, refill_id, "BUG: unexpected refill ID");
        assert!(!pending.quarantined, "BUG: refill is quarantined");
        pending
    }

    pub(super) fn complete_refill(&mut self, refill_id: u64) {
        let pending = self.pending_refill(refill_id);
        self.balance = self
            .balance
            .checked_add(pending.amount)
            .expect("BUG: operating balance overflow");
        self.completed_refills += 1;
        self.pending = None;
    }

    pub(super) fn fail_refill(&mut self, refill_id: u64) {
        self.pending_refill(refill_id);
        self.failed_refills += 1;
        self.pending = None;
    }

    pub(super) fn quarantine_refill(&mut self, refill_id: u64) {
        self.pending_refill(refill_id);
        self.pending.as_mut().unwrap().quarantined = true;
    }
}
