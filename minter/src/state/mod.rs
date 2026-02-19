#[cfg(test)]
mod tests;

use candid::Principal;
use cksol_types_internal::{Ed25519KeyName, InitArgs, UpgradeArgs};
use ic_ed25519::PublicKey;
use icrc_ledger_types::icrc1::account::Account;
use std::cell::RefCell;
use std::collections::BTreeSet;

pub mod audit;
pub mod event;

thread_local! {
    static STATE: RefCell<Option<State>> = RefCell::default();
}

pub fn read_state<R>(f: impl FnOnce(&State) -> R) -> R {
    STATE.with(|s| f(s.borrow().as_ref().expect("BUG: state is not initialized")))
}

pub fn init_once_state(state: State) {
    STATE.with(|s| {
        if s.borrow().is_some() {
            panic!("BUG: state is already initialized");
        }
        *s.borrow_mut() = Some(state);
    });
}

pub fn mutate_state<F, R>(f: F) -> R
where
    F: FnOnce(&mut State) -> R,
{
    STATE.with(|s| {
        f(s.borrow_mut()
            .as_mut()
            .expect("BUG: state is not initialized"))
    })
}

/// State of the minter.
///
/// # Design
///
/// The state is transient and not preserved across canister upgrades.
/// Relevant state changes are recorded in an append-only event log
/// (see [`crate::state::audit::process_event`]),
/// and replaying this log upon canister upgrade will re-create an equivalent state.
///
/// That means in particular:
/// * Methods mutating the state should generally not be accessible outside the state crate,
///   to ensure that the state is only mutating through events.
/// * Having public methods mutating the state may be acceptable for transient data (e.g. guards)
///   that do not need to be preserved across canister upgrades.
#[derive(Debug, PartialEq, Eq)]
pub struct State {
    minter_public_key: Option<SchnorrPublicKey>,
    master_key_name: Ed25519KeyName,
    ledger_canister_id: Principal,
    sol_rpc_canister_id: Principal,
    deposit_fee: u64,
    pending_update_balance_requests: BTreeSet<Account>,
}

impl State {
    pub fn minter_public_key(&self) -> Option<&SchnorrPublicKey> {
        self.minter_public_key.as_ref()
    }

    /// Set the minter public key only once.
    ///
    /// This is expected to happen only when the minter was freshly installed or after a canister upgrade.
    ///
    /// # Panics
    /// This method will panic if the public key was already set
    pub fn set_once_minter_public_key(&mut self, public_key: SchnorrPublicKey) {
        if self.minter_public_key.is_some() {
            panic!("BUG: minter public key is already set")
        }
        self.minter_public_key = Some(public_key);
    }

    pub fn sol_rpc_canister_id(&self) -> Principal {
        self.sol_rpc_canister_id
    }

    pub fn ledger_canister_id(&self) -> Principal {
        self.ledger_canister_id
    }

    pub fn master_key_name(&self) -> Ed25519KeyName {
        self.master_key_name
    }

    pub fn deposit_fee(&self) -> u64 {
        self.deposit_fee
    }

    pub fn pending_update_balance_requests_mut(&mut self) -> &mut BTreeSet<Account> {
        &mut self.pending_update_balance_requests
    }

    fn upgrade(
        &mut self,
        UpgradeArgs {
            sol_rpc_canister_id,
            deposit_fee,
        }: UpgradeArgs,
    ) -> Result<(), InvalidStateError> {
        if let Some(sol_rpc_canister_id) = sol_rpc_canister_id {
            self.sol_rpc_canister_id = sol_rpc_canister_id;
        }
        if let Some(deposit_fee) = deposit_fee {
            self.deposit_fee = deposit_fee;
        }
        Ok(())
    }
}

#[derive(Debug)]
pub enum InvalidStateError {
    InvalidCanisterId(String),
}

impl TryFrom<InitArgs> for State {
    type Error = InvalidStateError;

    fn try_from(
        InitArgs {
            sol_rpc_canister_id,
            ledger_canister_id,
            deposit_fee,
            master_key_name,
        }: InitArgs,
    ) -> Result<Self, Self::Error> {
        let canister_ids: BTreeSet<_> = [sol_rpc_canister_id, ledger_canister_id]
            .into_iter()
            .collect();
        if canister_ids.contains(&Principal::anonymous()) {
            return Err(InvalidStateError::InvalidCanisterId(
                "ERROR: anonymous principal is not accepted!".to_string(),
            ));
        }
        if canister_ids.len() < 2 {
            return Err(InvalidStateError::InvalidCanisterId(
                "ERROR: provided canister IDs are not distinct!".to_string(),
            ));
        }

        Ok(Self {
            minter_public_key: None,
            master_key_name,
            ledger_canister_id,
            sol_rpc_canister_id,
            deposit_fee,
            pending_update_balance_requests: BTreeSet::new(),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SchnorrPublicKey {
    pub public_key: PublicKey,
    pub chain_code: [u8; 32],
}
