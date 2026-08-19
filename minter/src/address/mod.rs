use crate::{
    runtime::CanisterRuntime,
    state::{SchnorrPublicKey, mutate_state, read_state},
};
use ic_cdk_management_canister::{SchnorrAlgorithm, SchnorrKeyId, SchnorrPublicKeyArgs};
use ic_ed25519::{DerivationIndex, DerivationPath as IcDerivationPath, PublicKey};
use icrc_ledger_types::icrc1::account::Account;
use solana_address::Address;

#[cfg(test)]
mod tests;

pub(crate) type DerivationPath = Vec<Vec<u8>>;

/// Implementation of the `get_deposit_address` canister endpoint.
/// Fetches the Schnorr master key lazily if not yet cached.
pub async fn get_deposit_address<R: CanisterRuntime>(runtime: &R, account: &Account) -> Address {
    let master_key = lazy_get_schnorr_master_key(runtime).await;
    account_address(&master_key, account)
}

pub fn minter_account<R: CanisterRuntime>(runtime: &R) -> Account {
    Account {
        owner: runtime.canister_self(),
        subaccount: None,
    }
}

pub fn minter_address<R: CanisterRuntime>(master_key: &SchnorrPublicKey, runtime: &R) -> Address {
    account_address(master_key, &minter_account(runtime))
}

pub fn account_address(master_key: &SchnorrPublicKey, account: &Account) -> Address {
    derive_public_key_from_account(master_key, account)
        .serialize_raw()
        .into()
}

pub async fn lazy_get_schnorr_master_key<R: CanisterRuntime>(runtime: &R) -> SchnorrPublicKey {
    if let Some(public_key) = read_state(|s| s.minter_public_key().cloned()) {
        return public_key;
    }

    let key_name = read_state(|s| s.master_key_name());

    let arg = SchnorrPublicKeyArgs {
        canister_id: None,
        derivation_path: vec![],
        key_id: SchnorrKeyId {
            algorithm: SchnorrAlgorithm::Ed25519,
            name: key_name.to_string(),
        },
    };
    let response = runtime.schnorr_public_key(arg).await;

    let schnorr_public_key = SchnorrPublicKey {
        public_key: PublicKey::deserialize_raw(response.public_key.as_slice())
            .expect("Failed to deserialize public key"),
        chain_code: response.chain_code.as_slice().try_into().unwrap(),
    };

    mutate_state(|s| s.set_once_minter_public_key(schnorr_public_key.clone()));
    schnorr_public_key
}

fn derive_public_key_from_account(
    master_public_key: &SchnorrPublicKey,
    account: &Account,
) -> PublicKey {
    derive_public_key(master_public_key, derivation_path(account))
}

pub(crate) fn derive_public_key(
    master_public_key: &SchnorrPublicKey,
    path: DerivationPath,
) -> PublicKey {
    let derivation_path = IcDerivationPath::new(path.into_iter().map(DerivationIndex).collect());
    let (public_key, _chain_code) = master_public_key
        .public_key
        .derive_subkey_with_chain_code(&derivation_path, &master_public_key.chain_code);
    public_key
}

pub(crate) fn derivation_path(account: &Account) -> DerivationPath {
    const SCHEMA_V1: u8 = 1;
    vec![
        vec![SCHEMA_V1],
        account.owner.as_slice().to_vec(),
        account.effective_subaccount().to_vec(),
    ]
}
