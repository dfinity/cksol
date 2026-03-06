use crate::state::{State, TaskType, mutate_state};
use cksol_types::{UpdateBalanceError, WithdrawSolError};
use icrc_ledger_types::icrc1::account::Account;
use std::{collections::BTreeSet, marker::PhantomData};

#[cfg(test)]
mod tests;

const MAX_CONCURRENT: usize = 100;

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

pub trait PendingRequests {
    fn pending_requests(state: &mut State) -> &mut BTreeSet<Account>;
}

pub struct PendingUpdateBalanceRequests;

impl PendingRequests for PendingUpdateBalanceRequests {
    fn pending_requests(state: &mut State) -> &mut BTreeSet<Account> {
        state.pending_update_balance_requests_mut()
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
    /// already a pending request for the specified [principal] or if there
    /// are at least [MAX_CONCURRENT] pending requests.
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

pub struct PendingWithdrawSolRequests;

impl PendingRequests for PendingWithdrawSolRequests {
    fn pending_requests(state: &mut State) -> &mut BTreeSet<Account> {
        state.pending_withdraw_sol_requests_mut()
    }
}

pub fn update_balance_guard(
    account: Account,
) -> Result<Guard<PendingUpdateBalanceRequests>, GuardError> {
    Guard::new(account)
}

pub fn withdraw_sol_guard(
    account: Account,
) -> Result<Guard<PendingWithdrawSolRequests>, GuardError> {
    Guard::new(account)
}

#[derive(Eq, PartialEq, Debug)]
pub enum TaskGuardError {
    AlreadyProcessing,
}

#[derive(Eq, PartialEq, Debug)]
pub struct TaskGuard {
    task: TaskType,
}

impl TaskGuard {
    pub fn new(task: TaskType) -> Result<Self, TaskGuardError> {
        mutate_state(|s| {
            if !s.active_tasks_mut().insert(task) {
                return Err(TaskGuardError::AlreadyProcessing);
            }
            Ok(Self { task })
        })
    }
}

impl Drop for TaskGuard {
    fn drop(&mut self) {
        mutate_state(|s| {
            s.active_tasks_mut().remove(&self.task);
        });
    }
}
