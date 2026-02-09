use crate::state::{mutate_state, read_state};
use candid::Principal;
use ic_cdk::management_canister::{
    SchnorrAlgorithm, SchnorrKeyId, SchnorrPublicKeyArgs, SchnorrPublicKeyResult,
    schnorr_public_key,
};
use ic_ed25519::{DerivationIndex, DerivationPath, PublicKey};
use icrc_ledger_types::icrc1::account::Account;
use icrc_ledger_types::icrc1::account::Subaccount;
use solana_address::Address;

pub async fn get_sol_address(principal: Principal, subaccount: Option<Subaccount>) -> Address {
    lazy_call_schnorr_public_key().await;

    let public_key = read_state(|s| {
        derive_public_key_from_account(
            &s.ed25519_public_key
                .clone()
                .expect("master key should be initialized"),
            &Account {
                owner: principal,
                subaccount,
            },
        )
    });

    Address::try_from(public_key.serialize_raw()).unwrap_or_else(|_| {
        panic!(
            "Expected Schnorr public key to be 32 bytes, but got: {:?} bytes",
            public_key
        )
    })
}

async fn lazy_call_schnorr_public_key() -> SchnorrPublicKeyResult {
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

fn derive_public_key_from_account(
    ed25519_public_key: &SchnorrPublicKeyResult,
    account: &Account,
) -> PublicKey {
    derive_public_key(ed25519_public_key, derivation_path(account))
}

fn derive_public_key(ed25519_public_key: &SchnorrPublicKeyResult, path: Vec<Vec<u8>>) -> PublicKey {
    let public_key = PublicKey::deserialize_raw(&ed25519_public_key.public_key.as_slice())
        .expect("Failed to deserialize public key");

    let derivation_path = DerivationPath::new(path.into_iter().map(DerivationIndex).collect());
    let (public_key, _chain_code) = public_key.derive_subkey_with_chain_code(
        &derivation_path,
        &ed25519_public_key.chain_code.as_slice().try_into().unwrap(),
    );

    public_key
}

fn derivation_path(account: &Account) -> Vec<Vec<u8>> {
    const SCHEMA_V1: u8 = 1;
    let mut result = vec![vec![SCHEMA_V1]];
    result.push(account.owner.as_slice().to_vec());
    result.push(account.effective_subaccount().to_vec());
    result
}
