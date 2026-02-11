use candid::Principal;
use canlog::LogFilter;
use cksol_types::Ed25519KeyName;
use ic_ed25519::PublicKey;
use sol_rpc_client::SOL_RPC_CANISTER;
use sol_rpc_types::Lamport;
use std::{
    cell::RefCell,
    ops::{Deref, DerefMut},
};

thread_local! {
    pub static STATE: RefCell<State> = RefCell::default();
}

pub fn read_state<R>(f: impl FnOnce(&State) -> R) -> R {
    STATE.with(|s| f(s.borrow().deref()))
}

pub fn mutate_state<F, R>(f: F) -> R
where
    F: FnOnce(&mut State) -> R,
{
    STATE.with(|s| f(s.borrow_mut().deref_mut()))
}

#[derive(Debug, PartialEq, Eq)]
pub struct State {
    pub master_public_key: Option<SchnorrPublicKey>,
    pub master_key_name: Ed25519KeyName,
    pub sol_rpc_canister_id: Principal,
    pub ledger_canister_id: Principal,
    pub deposit_fee: Lamport,
    pub log_filter: LogFilter,
}

impl Default for State {
    fn default() -> Self {
        // 10 million lamports = 0.01 SOL
        const DEFAULT_DEPOSIT_FEE: Lamport = 10_000_000;
        Self {
            master_public_key: None,
            master_key_name: Ed25519KeyName::default(),
            sol_rpc_canister_id: SOL_RPC_CANISTER,
            ledger_canister_id: Principal::anonymous(),
            deposit_fee: DEFAULT_DEPOSIT_FEE,
            log_filter: LogFilter::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SchnorrPublicKey {
    pub public_key: PublicKey,
    pub chain_code: [u8; 32],
}
