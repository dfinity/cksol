use crate::state::{State, TaskType, mutate_state};
use cksol_types::{DepositSolError, DepositSplError, WithdrawSolError};
use icrc_ledger_types::icrc1::account::Account;
use solana_address::Address;
use std::{collections::BTreeSet, marker::PhantomData};

#[cfg(test)]
mod tests;

const MAX_CONCURRENT: usize = 100;

#[derive(Eq, PartialEq, Debug)]
pub enum GuardError {
    AlreadyProcessing,
    TooManyConcurrentRequests,
}

impl From<GuardError> for DepositSolError {
    fn from(e: GuardError) -> Self {
        match e {
            GuardError::AlreadyProcessing => Self::AlreadyProcessing,
            GuardError::TooManyConcurrentRequests => {
                Self::TemporarilyUnavailable("too many concurrent requests".to_string())
            }
        }
    }
}

impl From<GuardError> for DepositSplError {
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
    type Key: Clone + Ord;

    fn pending_requests(state: &mut State) -> &mut BTreeSet<Self::Key>;
}

/// Guards a request key from executing twice and limits the operation to
/// [`MAX_CONCURRENT`] requests in parallel.
#[must_use]
pub struct Guard<R: PendingRequests> {
    key: R::Key,
    _marker: PhantomData<R>,
}

impl<R: PendingRequests> Guard<R> {
    /// Attempts to create a new guard. Fails if the key already has a pending
    /// request or there are at least [`MAX_CONCURRENT`] requests for this operation.
    pub fn new(key: R::Key) -> Result<Self, GuardError> {
        mutate_state(|s| {
            let requests = R::pending_requests(s);
            if requests.contains(&key) {
                return Err(GuardError::AlreadyProcessing);
            }
            if requests.len() >= MAX_CONCURRENT {
                return Err(GuardError::TooManyConcurrentRequests);
            }
            requests.insert(key.clone());
            Ok(Self {
                key,
                _marker: PhantomData,
            })
        })
    }
}

impl<R: PendingRequests> Drop for Guard<R> {
    fn drop(&mut self) {
        mutate_state(|s| R::pending_requests(s).remove(&self.key));
    }
}

pub struct PendingDepositSolRequests;

impl PendingRequests for PendingDepositSolRequests {
    type Key = Account;

    fn pending_requests(state: &mut State) -> &mut BTreeSet<Account> {
        state.pending_deposit_sol_request_guards_mut()
    }
}

pub struct PendingDepositSplRequests;

impl PendingRequests for PendingDepositSplRequests {
    type Key = (Account, Address);

    fn pending_requests(state: &mut State) -> &mut BTreeSet<Self::Key> {
        state.pending_deposit_spl_request_guards_mut()
    }
}

pub struct PendingWithdrawalRequests;

impl PendingRequests for PendingWithdrawalRequests {
    type Key = Account;

    fn pending_requests(state: &mut State) -> &mut BTreeSet<Account> {
        state.pending_withdrawal_request_guards_mut()
    }
}

pub fn deposit_sol_guard(account: Account) -> Result<Guard<PendingDepositSolRequests>, GuardError> {
    Guard::new(account)
}

pub fn deposit_spl_guard(
    account: Account,
    mint: Address,
) -> Result<Guard<PendingDepositSplRequests>, GuardError> {
    Guard::new((account, mint))
}

pub fn withdrawal_guard(account: Account) -> Result<Guard<PendingWithdrawalRequests>, GuardError> {
    Guard::new(account)
}

#[derive(Eq, PartialEq, Debug)]
pub enum TimerGuardError {
    AlreadyProcessing,
}

#[derive(Eq, PartialEq, Debug)]
pub struct TimerGuard {
    task: TaskType,
}

impl TimerGuard {
    pub fn new(task: TaskType) -> Result<Self, TimerGuardError> {
        mutate_state(|s| {
            if !s.active_tasks_mut().insert(task) {
                return Err(TimerGuardError::AlreadyProcessing);
            }
            Ok(Self { task })
        })
    }
}

impl Drop for TimerGuard {
    fn drop(&mut self) {
        mutate_state(|s| {
            s.active_tasks_mut().remove(&self.task);
        });
    }
}
