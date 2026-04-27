use crate::{
    constants::MAX_CONCURRENT_HTTP_OUTCALLS,
    state::{State, TaskType, mutate_state},
};
use cksol_types::{ProcessDepositError, WithdrawalError};
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

impl From<GuardError> for ProcessDepositError {
    fn from(e: GuardError) -> Self {
        match e {
            GuardError::AlreadyProcessing => Self::AlreadyProcessing,
            GuardError::TooManyConcurrentRequests => {
                Self::TemporarilyUnavailable("too many concurrent requests".to_string())
            }
        }
    }
}

impl From<GuardError> for WithdrawalError {
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

pub struct PendingProcessDepositRequests;

impl PendingRequests for PendingProcessDepositRequests {
    fn pending_requests(state: &mut State) -> &mut BTreeSet<Account> {
        state.pending_process_deposit_request_guards_mut()
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

pub struct PendingWithdrawalRequests;

impl PendingRequests for PendingWithdrawalRequests {
    fn pending_requests(state: &mut State) -> &mut BTreeSet<Account> {
        state.pending_withdrawal_request_guards_mut()
    }
}

pub fn process_deposit_guard(
    account: Account,
) -> Result<Guard<PendingProcessDepositRequests>, GuardError> {
    Guard::new(account)
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

#[derive(Debug, Eq, PartialEq)]
pub enum HttpOutcallGuardError {
    TooManyOutcalls,
}

/// Guards a single HTTP outcall to the SOL RPC canister.
///
/// Acquiring this guard increments the active-outcall counter in canister state;
/// dropping it decrements the counter. [`HttpOutcallGuard::new`] fails with
/// [`HttpOutcallGuardError::TooManyOutcalls`] when
/// [`MAX_CONCURRENT_HTTP_OUTCALLS`] guards are already held.
#[must_use]
pub struct HttpOutcallGuard;

impl HttpOutcallGuard {
    pub fn new() -> Result<Self, HttpOutcallGuardError> {
        mutate_state(|s| {
            if s.active_http_outcalls() >= MAX_CONCURRENT_HTTP_OUTCALLS {
                return Err(HttpOutcallGuardError::TooManyOutcalls);
            }
            *s.active_http_outcalls_mut() += 1;
            Ok(Self)
        })
    }
}

impl Drop for HttpOutcallGuard {
    fn drop(&mut self) {
        mutate_state(|s| {
            let count = s.active_http_outcalls_mut();
            *count = count
                .checked_sub(1)
                .expect("BUG: HTTP outcall counter underflow");
        });
    }
}
