use ic_cdk::management_canister::SchnorrPublicKeyResult;
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
    pub ed25519_public_key: Option<SchnorrPublicKeyResult>,
    pub ed25519_key_name: String,
}

impl Default for State {
    fn default() -> Self {
        Self {
            ed25519_public_key: Default::default(),
            ed25519_key_name: "dfx_test_key".to_string(),
        }
    }
}
