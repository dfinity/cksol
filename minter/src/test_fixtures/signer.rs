use super::runtime::Stubs;
use crate::{address::DerivationPath, signer::SchnorrSigner};
use ic_cdk::management_canister::SignCallError;

#[derive(Clone, Default)]
pub struct MockSchnorrSigner {
    responses: Stubs<Result<Vec<u8>, SignCallError>>,
}

impl MockSchnorrSigner {
    pub fn with_signatures(
        signatures: impl IntoIterator<Item = [u8; 64], IntoIter: Send + 'static>,
    ) -> Self {
        Self {
            responses: signatures.into_iter().map(|sig| Ok(sig.to_vec())).into(),
        }
    }

    pub fn with_responses(
        responses: impl IntoIterator<Item = Result<Vec<u8>, SignCallError>, IntoIter: Send + 'static>,
    ) -> Self {
        Self {
            responses: responses.into_iter().into(),
        }
    }

    pub fn add_signature(mut self, signature: [u8; 64]) -> Self {
        self.responses = self.responses.add(Ok(signature.to_vec()));
        self
    }

    pub fn add_response(mut self, response: Result<Vec<u8>, SignCallError>) -> Self {
        self.responses = self.responses.add(response);
        self
    }
}

impl SchnorrSigner for MockSchnorrSigner {
    async fn sign(
        &self,
        _message: Vec<u8>,
        _derivation_path: DerivationPath,
    ) -> Result<Vec<u8>, SignCallError> {
        self.responses.next()
    }
}
