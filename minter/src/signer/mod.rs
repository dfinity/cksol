use crate::{address::DerivationPath, state::read_state};
use ic_cdk::management_canister::{
    SchnorrAlgorithm, SchnorrKeyId, SignCallError, SignWithSchnorrArgs, sign_with_schnorr,
};

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
