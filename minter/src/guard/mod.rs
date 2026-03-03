use crate::state::{State, mutate_state};
use cksol_types::{UpdateBalanceError, WithdrawSolError};
use icrc_ledger_types::icrc1::account::Account;
use std::{collections::BTreeSet, marker::PhantomData};

#[cfg(test)]
mod tests;

const MAX_CONCURRENT: usize = 100;

pub fn update_balance_guard(
    account: Account,
) -> Result<Guard<PendingUpdateBalanceRequests>, GuardError> {
    Guard::new(account)
}

pub struct PendingUpdateBalanceRequests;

impl PendingRequests for PendingUpdateBalanceRequests {
    fn pending_requests(state: &mut State) -> &mut BTreeSet<Account> {
        state.pending_update_balance_requests_mut()
    }
}

pub fn withdraw_sol_guard(
    account: Account,
) -> Result<Guard<PendingWithdrawSolRequests>, GuardError> {
    Guard::new(account)
}

pub struct PendingWithdrawSolRequests;

impl PendingRequests for PendingWithdrawSolRequests {
    fn pending_requests(state: &mut State) -> &mut BTreeSet<Account> {
        state.pending_withdraw_sol_requests_mut()
    }
}

/// Guards a block from executing twice when called by the same user and from being
/// executed [`MAX_CONCURRENT`] or more times in parallel.
#[must_use]
pub struct Guard<R: PendingRequests> {
    account: Account,
    _marker: PhantomData<R>,
}

impl<R: PendingRequests> Guard<R> {
    /// Attempts to create a new guard for the current block. Fails if there is
    /// already a pending request for the specified [`Account`] or if there
    /// are at least [`MAX_CONCURRENT`] pending requests.
    pub fn new(account: Account) -> Result<Self, GuardError> {
        mutate_state(|s| {
            let accounts = R::pending_requests(s);
            if accounts.contains(&account) {
                return Err(GuardError::AlreadyProcessing);
            }
            if accounts.len() >= MAX_CONCURRENT {
                return Err(GuardError::TooManyConcurrentRequests);
            }
            accounts.insert(account);
            Ok(Self {
                account,
                _marker: PhantomData,
            })
        })
    }
}

impl<R: PendingRequests> Drop for Guard<R> {
    fn drop(&mut self) {
        mutate_state(|s| R::pending_requests(s).remove(&self.account));
    }
}

pub trait PendingRequests {
    fn pending_requests(state: &mut State) -> &mut BTreeSet<Account>;
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
