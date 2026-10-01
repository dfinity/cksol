use crate::{
    runtime::CanisterRuntime,
    state::{SchnorrPublicKey, audit::process_event, event::EventType, mutate_state, read_state},
};
use ic_cdk_management_canister::{SchnorrAlgorithm, SchnorrKeyId, SchnorrPublicKeyArgs};
use ic_ed25519::{DerivationIndex, DerivationPath as IcDerivationPath, PublicKey};
use icrc_ledger_types::icrc1::account::Account;
use solana_address::Address;
use thiserror::Error;

#[cfg(test)]
mod tests;

pub(crate) type DerivationPath = Vec<Vec<u8>>;

/// Derivation path of the minter's main address: the empty path, i.e. the master key
/// itself. Account paths always start with the schema tag (see [`derivation_path`]),
/// so the main address can never collide with any deposit address.
pub const MINTER_DERIVATION_PATH: DerivationPath = Vec::new();

/// Implementation of the `get_deposit_address` canister endpoint.
/// Because the endpoint is a query, it must be synchronous and cannot fetch the
/// master key on demand — it traps if the key has not yet been initialized.
pub fn get_deposit_address(account: &Account) -> Address {
    let master_key = read_state(|s| s.minter_public_key().cloned())
        .unwrap_or_else(|| ic_cdk::trap("master key not yet initialized"));
    account_address(&master_key, account)
}

/// The minter's main Solana address, holding the consolidated funds:
/// the master public key itself, on [`MINTER_DERIVATION_PATH`].
pub fn minter_address(master_key: &SchnorrPublicKey) -> Address {
    master_key.public_key.serialize_raw().into()
}

pub fn account_address(master_key: &SchnorrPublicKey, account: &Account) -> Address {
    derive_public_key_from_account(master_key, account)
        .serialize_raw()
        .into()
}

/// The minter's Schnorr master public key is only unavailable in the short window
/// between the first initialization of the minter and the completion of the
/// key fetch scheduled by `init`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Error)]
#[error("the minter public key is not yet available, try again later")]
pub struct MinterPublicKeyNotYetAvailable;

pub fn minter_public_key() -> Result<SchnorrPublicKey, MinterPublicKeyNotYetAvailable> {
    read_state(|s| s.minter_public_key().cloned()).ok_or(MinterPublicKeyNotYetAvailable)
}

/// Fetches the minter's Schnorr master public key and records it in a
/// [`EventType::MinterPublicKeyFetched`] event, so that replaying the event log
/// makes the key available without any asynchronous fetch.
///
/// Returns without fetching once the key is available, so the event is recorded
/// at most once over the lifetime of the minter. Concurrent calls may each fetch
/// the key, but only the first one records the event.
pub async fn fetch_and_record_minter_public_key<R: CanisterRuntime>(runtime: &R) {
    if read_state(|s| s.minter_public_key().is_some()) {
        return;
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

    let public_key = PublicKey::deserialize_raw(response.public_key.as_slice())
        .expect("the management canister returns a valid Ed25519 public key");
    let chain_code = response
        .chain_code
        .as_slice()
        .try_into()
        .expect("the management canister returns a 32-byte chain code");

    mutate_state(|state| {
        if state.minter_public_key().is_some() {
            return;
        }
        process_event(
            state,
            EventType::MinterPublicKeyFetched {
                public_key,
                chain_code,
            },
            runtime,
        );
    });
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
