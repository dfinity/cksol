use crate::{
    address::{DerivationPath, derivation_path},
    signer::SchnorrSigner,
};
use ic_cdk_management_canister::SignCallError;
use icrc_ledger_types::icrc1::account::Account;
use mockall::mock;
use sha2::{Digest, Sha512};
use solana_signature::Signature;
use std::{
    collections::BTreeMap,
    sync::{Arc, OnceLock},
};

/// The [`Signature`] the mock signer answers with the `occurrence`-th time
/// `derivation_path` signs, as the hash of both. Hashing every component under its own
/// length keeps the mapping injective, so no two accounts and no two signatures by the same
/// account collide.
pub(super) fn derivation_path_signature(
    derivation_path: &DerivationPath,
    occurrence: usize,
) -> Signature {
    let mut hasher = Sha512::new();
    hasher.update((derivation_path.len() as u64).to_le_bytes());
    for component in derivation_path {
        hasher.update((component.len() as u64).to_le_bytes());
        hasher.update(component);
    }
    hasher.update((occurrence as u64).to_le_bytes());

    Signature::try_from(hasher.finalize().as_slice()).expect("BUG: SHA-512 is 64 bytes wide")
}

/// How the mock signer answers one expected signing request.
#[derive(Clone)]
pub enum ExpectedSignature {
    /// Answer with the signature derived from the signing account, which the test reads
    /// back with `account_signature` or `account_signature_nth`.
    Derived,
    /// Answer with this signature, for a test that cannot derive the one it needs.
    Exactly(Signature),
    /// Fail the signing request.
    Failing(SignCallError),
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

/// A [`SchnorrSigner`] that answers only the signing requests a test has registered with
/// [`Self::add_signature`], in registration order per account.
///
/// There is no default answer: signing without a registration fails the test, and so does a
/// registration that is never used. An account expected to sign twice is registered twice,
/// and its two [`ExpectedSignature::Derived`] answers are its first and second signatures.
#[derive(Clone, Default)]
pub struct MockSchnorrSigner {
    expectations: Vec<(DerivationPath, ExpectedSignature)>,
    mock: Arc<OnceLock<MockSigner>>,
}

impl MockSchnorrSigner {
    pub fn add_signature(mut self, account: &Account, signature: ExpectedSignature) -> Self {
        assert!(
            self.mock.get().is_none(),
            "BUG: register all expected signatures before the first signing request"
        );
        self.expectations
            .push((derivation_path(account), signature));
        self
    }

    fn mock(&self) -> &MockSigner {
        self.mock.get_or_init(|| {
            let mut mock = MockSigner::new();
            let mut occurrences: BTreeMap<&DerivationPath, usize> = BTreeMap::new();

            for (derivation_path, expected) in &self.expectations {
                let occurrence = occurrences.entry(derivation_path).or_default();
                let response = match expected {
                    ExpectedSignature::Derived => {
                        Ok(derivation_path_signature(derivation_path, *occurrence))
                    }
                    ExpectedSignature::Exactly(signature) => Ok(*signature),
                    ExpectedSignature::Failing(error) => Err(error.clone()),
                };
                *occurrence += 1;

                let expected_path = derivation_path.clone();
                mock.expect_sign()
                    .withf(move |_message, path| path == &expected_path)
                    .times(1)
                    .return_once(move |_message, _path| {
                        response.map(|signature| signature.as_ref().to_vec())
                    });
            }
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
