use candid::Principal;
use cksol_types::GetSolAddressArgs;
use ic_cdk::management_canister::{
    SchnorrAlgorithm, SchnorrKeyId, SchnorrPublicKeyArgs, SchnorrPublicKeyResult,
    schnorr_public_key,
};
use ic_ed25519::{DerivationIndex, DerivationPath, PublicKey};
use icrc_ledger_types::icrc1::account::Account;
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

/// Returns the derivation path that should be used to sign a message from a
/// specified account.
pub fn derivation_path(account: &Account) -> Vec<Vec<u8>> {
    const SCHEMA_V1: u8 = 1;
    let mut result = vec![vec![SCHEMA_V1]];
    result.push(account.owner.as_slice().to_vec());
    result.push(account.effective_subaccount().to_vec());
    result
}

pub async fn get_sol_address(args: GetSolAddressArgs) -> String {
    let owner = args.owner.unwrap_or_else(ic_cdk::api::msg_caller);
    assert_ne!(
        owner,
        Principal::anonymous(),
        "the owner must be non-anonymous"
    );

    lazy_call_schnorr_public_key().await;

    read_state(|s| {
        account_to_address_from_state(
            s,
            &Account {
                owner,
                subaccount: args.subaccount,
            },
        )
    })
}

pub fn account_to_address_from_state(s: &State, account: &Account) -> String {
    account_to_address(
        s.ed25519_public_key
            .as_ref()
            .expect("bug: public key must be initialized"),
        account,
    )
}

pub fn account_to_address(
    ed25519_public_key: &SchnorrPublicKeyResult,
    account: &Account,
) -> String {
    bs58::encode(&derive_public_key_from_account(ed25519_public_key, account)).into_string()
}

pub fn derive_public_key_from_account(
    ed25519_public_key: &SchnorrPublicKeyResult,
    account: &Account,
) -> PublicKey {
    derive_public_key(ed25519_public_key, derivation_path(account))
}

pub fn derive_public_key(
    ed25519_public_key: &SchnorrPublicKeyResult,
    path: Vec<Vec<u8>>,
) -> PublicKey {
    let public_key = PublicKey::deserialize_raw(&ed25519_public_key.public_key.as_slice())
        .expect("Failed to deserialize public key");

    let derivation_path = DerivationPath::new(path.into_iter().map(DerivationIndex).collect());
    let (public_key, _chain_code) = public_key.derive_subkey_with_chain_code(
        &derivation_path,
        &ed25519_public_key.chain_code.as_slice().try_into().unwrap(),
    );

    public_key
}
