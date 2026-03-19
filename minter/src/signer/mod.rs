use crate::{address::DerivationPath, state::read_state};
use ic_cdk::management_canister::{
    SchnorrAlgorithm, SchnorrKeyId, SignCallError, SignWithSchnorrArgs, sign_with_schnorr,
};
use solana_signature::Signature;

pub trait SchnorrSigner {
    fn sign(
        &self,
        message: Vec<u8>,
        derivation_path: DerivationPath,
    ) -> impl Future<Output = Result<Vec<u8>, SignCallError>>;
}

/// Production signer that delegates to the IC management canister.
#[derive(Clone, Default)]
pub struct IcSchnorrSigner;

impl SchnorrSigner for IcSchnorrSigner {
    async fn sign(
        &self,
        message: Vec<u8>,
        derivation_path: DerivationPath,
    ) -> Result<Vec<u8>, SignCallError> {
        let key_name = read_state(|s| s.master_key_name());
        let args = SignWithSchnorrArgs {
            message,
            derivation_path,
            key_id: SchnorrKeyId {
                algorithm: SchnorrAlgorithm::Ed25519,
                name: key_name.to_string(),
            },
            aux: None,
        };
        let response = sign_with_schnorr(&args).await?;
        Ok(response.signature)
    }
}

pub async fn sign_bytes(
    derivation_paths: impl IntoIterator<Item = DerivationPath>,
    signer: &impl SchnorrSigner,
    bytes: Vec<u8>,
) -> Result<Vec<Signature>, SignCallError> {
    fn signature_from_bytes(bytes: Vec<u8>) -> Signature {
        <[u8; 64]>::try_from(bytes.as_slice())
            .unwrap_or_else(|_| {
                panic!("BUG: expected 64-byte signature, got {} bytes", bytes.len())
            })
            .into()
    }
    let futures = derivation_paths
        .into_iter()
        .map(|derivation_path| signer.sign(bytes.clone(), derivation_path));
    let signatures = futures::future::try_join_all(futures)
        .await?
        .into_iter()
        .map(signature_from_bytes)
        .collect();
    Ok(signatures)
}
