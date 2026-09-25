use crate::{
    address::{DerivationPath, derivation_path},
    signer::SchnorrSigner,
};
use ic_cdk_management_canister::SignCallError;
use icrc_ledger_types::icrc1::account::Account;
use mockall::mock;
use solana_signature::Signature;
use std::sync::{Arc, OnceLock};

/// The [`Signature`] the mock signer returns for `derivation_path` unless the test
/// registers an override, laid out so that the owner and the subaccount of the signing
/// account are both readable in the signature bytes.
pub(super) fn derivation_path_signature(derivation_path: &DerivationPath) -> Signature {
    const MAX_PRINCIPAL_LEN: usize = 29;
    let [_schema_version, owner, subaccount] = derivation_path.as_slice() else {
        panic!("BUG: unexpected derivation path {derivation_path:?}");
    };
    let mut bytes = [0_u8; 64];
    bytes[..owner.len()].copy_from_slice(owner);
    bytes[MAX_PRINCIPAL_LEN..MAX_PRINCIPAL_LEN + subaccount.len()].copy_from_slice(subaccount);
    Signature::from(bytes)
}

mock! {
    Signer {}

    impl SchnorrSigner for Signer {
        async fn sign(
            &self,
            message: Vec<u8>,
            derivation_path: DerivationPath,
        ) -> Result<Vec<u8>, SignCallError>;
    }
}

/// A [`SchnorrSigner`] that answers every signing request with
/// [`derivation_path_signature`], so the signature a transaction carries follows from which
/// account signed it.
///
/// Tests that need a different answer register it up front with [`Self::signing_for`] or
/// [`Self::failing_to_sign_for`]. Overrides are consumed in registration order and must all
/// be used, so an account registered twice signs twice.
#[derive(Clone, Default)]
pub struct MockSchnorrSigner {
    overrides: Vec<(DerivationPath, Result<Vec<u8>, SignCallError>)>,
    mock: Arc<OnceLock<MockSigner>>,
}

impl MockSchnorrSigner {
    pub fn signing_for(self, account: &Account, signature: Signature) -> Self {
        self.overriding(account, Ok(signature.as_ref().to_vec()))
    }

    pub fn failing_to_sign_for(self, account: &Account, error: SignCallError) -> Self {
        self.overriding(account, Err(error))
    }

    fn overriding(mut self, account: &Account, response: Result<Vec<u8>, SignCallError>) -> Self {
        assert!(
            self.mock.get().is_none(),
            "BUG: register all signing overrides before the first signing request"
        );
        self.overrides.push((derivation_path(account), response));
        self
    }

    fn mock(&self) -> &MockSigner {
        self.mock.get_or_init(|| {
            let mut mock = MockSigner::new();
            for (derivation_path, response) in self.overrides.clone() {
                mock.expect_sign()
                    .withf(move |_message, path| path == &derivation_path)
                    .times(1)
                    .return_once(move |_message, _path| response);
            }
            mock.expect_sign()
                .returning(|_message, path| Ok(derivation_path_signature(&path).as_ref().to_vec()));
            mock
        })
    }
}

impl SchnorrSigner for MockSchnorrSigner {
    async fn sign(
        &self,
        message: Vec<u8>,
        derivation_path: DerivationPath,
    ) -> Result<Vec<u8>, SignCallError> {
        self.mock().sign(message, derivation_path).await
    }
}
