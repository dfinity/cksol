use crate::state::{State, mutate_state};
use cksol_types::{UpdateBalanceError, WithdrawSolError};
use icrc_ledger_types::icrc1::account::Account;
use std::{collections::BTreeSet, marker::PhantomData};

#[cfg(test)]
mod tests;

const MAX_CONCURRENT: usize = 100;

pub fn update_balance_guard(
    account: Account,
) -> Result<Guard<Account, PendingUpdateBalanceRequests>, GuardError> {
    Guard::new(account)
}

pub struct PendingUpdateBalanceRequests;

impl GetLocked<Account> for PendingUpdateBalanceRequests {
    fn get_locked(state: &mut State) -> &mut BTreeSet<Account> {
        state.pending_update_balance_requests_mut()
    }
}

pub fn withdraw_sol_guard(
    account: Account,
) -> Result<Guard<Account, PendingWithdrawSolRequests>, GuardError> {
    Guard::new(account)
}

pub struct PendingWithdrawSolRequests;

impl GetLocked<Account> for PendingWithdrawSolRequests {
    fn get_locked(state: &mut State) -> &mut BTreeSet<Account> {
        state.pending_withdraw_sol_requests_mut()
    }
}

/// Guards a block from executing twice when called by the same user and from being
/// executed [`MAX_CONCURRENT`] or more times in parallel.
#[must_use]
pub struct Guard<T: Ord, G: GetLocked<T>> {
    value: T,
    _marker: PhantomData<G>,
}

impl<T: Ord + Clone, G: GetLocked<T>> Guard<T, G> {
    /// Attempts to create a new guard for the current block. Fails if there is
    /// already a pending request for the specified [`Account`] or if there
    /// are at least [`MAX_CONCURRENT`] pending requests.
    pub fn new(value: T) -> Result<Self, GuardError> {
        mutate_state(|s| {
            let already_locked = G::get_locked(s);
            if already_locked.contains(&value) {
                return Err(GuardError::AlreadyProcessing);
            }
            if already_locked.len() >= MAX_CONCURRENT {
                return Err(GuardError::TooManyConcurrentRequests);
            }
            already_locked.insert(value.clone());
            Ok(Self {
                value,
                _marker: PhantomData,
            })
        })
    }
}

impl<T: Ord, G: GetLocked<T>> Drop for Guard<T, G> {
    fn drop(&mut self) {
        mutate_state(|s| G::get_locked(s).remove(&self.value));
    }
}

pub trait GetLocked<T: Ord> {
    fn get_locked(state: &mut State) -> &mut BTreeSet<T>;
}

#[derive(Eq, PartialEq, Debug)]
pub enum GuardError {
    AlreadyProcessing,
    TooManyConcurrentRequests,
}

impl From<GuardError> for UpdateBalanceError {
    fn from(e: GuardError) -> Self {
        match e {
            GuardError::AlreadyProcessing => Self::AlreadyProcessing,
            GuardError::TooManyConcurrentRequests => {
                Self::TemporarilyUnavailable("too many concurrent requests".to_string())
            }
        }
    }
}

impl From<GuardError> for WithdrawSolError {
    fn from(e: GuardError) -> Self {
        match e {
            GuardError::AlreadyProcessing => Self::AlreadyProcessing,
            GuardError::TooManyConcurrentRequests => {
                Self::TemporarilyUnavailable("too many concurrent requests".to_string())
            }
        }
    }
}
