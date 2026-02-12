#[cfg(test)]
mod tests;

use crate::state::SchnorrPublicKey;
use crate::state::{mutate_state, read_state};
use candid::Principal;
use ic_cdk::management_canister::{
    SchnorrAlgorithm, SchnorrKeyId, SchnorrPublicKeyArgs, schnorr_public_key,
};
use ic_ed25519::{DerivationIndex, DerivationPath, PublicKey};
use icrc_ledger_types::icrc1::account::Account;
use icrc_ledger_types::icrc1::account::Subaccount;
use solana_address::Address;

pub async fn get_deposit_address(principal: Principal, subaccount: Option<Subaccount>) -> Address {
    let master_public_key = lazy_get_schnorr_master_key().await;

    let public_key = derive_public_key_from_account(
        &master_public_key,
        &Account {
            owner: principal,
            subaccount,
        },
    );

    Address::from(public_key.serialize_raw())
}

async fn lazy_get_schnorr_master_key() -> SchnorrPublicKey {
    if let Some(public_key) = read_state(|s| s.master_public_key.clone()) {
        return public_key;
    }

    let key_name = read_state(|s| s.master_key_name.to_string());

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
        .expect("failed to obtain the canister master key");

    let public_key = PublicKey::deserialize_raw(response.public_key.as_slice())
        .expect("Failed to deserialize public key");
    let schnorr_public_key = SchnorrPublicKey {
        public_key,
        chain_code: response.chain_code.as_slice().try_into().unwrap(),
    };

    mutate_state(|s| s.master_public_key = Some(schnorr_public_key.clone()));
    schnorr_public_key
}

fn derive_public_key_from_account(
    master_public_key: &SchnorrPublicKey,
    account: &Account,
) -> PublicKey {
    derive_public_key(master_public_key, derivation_path(account))
}

fn derive_public_key(master_public_key: &SchnorrPublicKey, path: Vec<Vec<u8>>) -> PublicKey {
    let derivation_path = DerivationPath::new(path.into_iter().map(DerivationIndex).collect());
    let (public_key, _chain_code) = master_public_key
        .public_key
        .derive_subkey_with_chain_code(&derivation_path, &master_public_key.chain_code);

    public_key
}

fn derivation_path(account: &Account) -> Vec<Vec<u8>> {
    const SCHEMA_V1: u8 = 1;
    vec![
        vec![SCHEMA_V1],
        account.owner.as_slice().to_vec(),
        account.effective_subaccount().to_vec(),
    ]
}
