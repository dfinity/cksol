use ic_cdk::management_canister::{
    SchnorrAlgorithm, SchnorrKeyId, SchnorrPublicKeyArgs, SchnorrPublicKeyResult,
    schnorr_public_key,
};
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

#[derive(Debug, Default, PartialEq, Eq)]
pub struct State {
    ed25519_public_key: Option<SchnorrPublicKeyResult>,
    ed25519_key_name: String,
}

pub async fn lazy_call_schnorr_public_key() -> SchnorrPublicKeyResult {
    if let Some(public_key) = read_state(|s| s.ed25519_public_key.clone()) {
        return public_key;
    }

    let key_name = read_state(|s| s.ed25519_key_name.clone());

    let arg = SchnorrPublicKeyArgs {
        canister_id: None,
        derivation_path: vec![],
        key_id: SchnorrKeyId {
            algorithm: SchnorrAlgorithm::Ed25519,
            name: key_name,
        },
    };
    let response = schnorr_public_key(&arg)
        .await
        .expect("failed to obtain the key");

    mutate_state(|s| s.ed25519_public_key = Some(response.clone()));
    response
}
