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

/// Expects `account` to sign once, answering with the signature derived from it, which the
/// test reads back with `account_signature`.
///
/// Use [`SignerExpectation::times`] for an account that signs repeatedly, each time
/// answered with its next derived signature, or [`SignerExpectation::expect`] to spell out
/// the exact sequence of answers.
pub fn sign_for(account: &Account) -> SignerExpectation {
    SignerExpectation {
        derivation_path: derivation_path(account),
        answers: Answers::Derived(1),
    }
}

/// What one account is expected to be asked to sign, and how the mock signer answers.
#[derive(Clone)]
pub struct SignerExpectation {
    derivation_path: DerivationPath,
    answers: Answers,
}

impl SignerExpectation {
    /// Expects `count` signing requests, each answered with the account's next derived
    /// signature, so no two of them are alike.
    pub fn times(mut self, count: usize) -> Self {
        self.answers = Answers::Derived(count);
        self
    }

    /// Expects one signing request per given answer, in order.
    pub fn expect(
        mut self,
        answers: impl IntoIterator<Item = Result<Signature, SignCallError>>,
    ) -> Self {
        self.answers = Answers::Given(answers.into_iter().collect());
        self
    }

    fn signatures(&self, first_occurrence: usize) -> Vec<Result<Signature, SignCallError>> {
        match &self.answers {
            Answers::Derived(count) => (0..*count)
                .map(|index| {
                    Ok(derivation_path_signature(
                        &self.derivation_path,
                        first_occurrence + index,
                    ))
                })
                .collect(),
            Answers::Given(answers) => answers.clone(),
        }
    }
}

#[derive(Clone)]
enum Answers {
    Derived(usize),
    Given(Vec<Result<Signature, SignCallError>>),
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
/// [`Self::add_signer`], in registration order per account.
///
/// There is no default answer: signing without an expectation fails the test, and so does an
/// expectation that goes unused.
#[derive(Clone, Default)]
pub struct MockSchnorrSigner {
    expectations: Vec<SignerExpectation>,
    mock: Arc<OnceLock<MockSigner>>,
}

impl MockSchnorrSigner {
    pub fn add_signer(mut self, expectation: SignerExpectation) -> Self {
        assert!(
            self.mock.get().is_none(),
            "BUG: register all expected signers before the first signing request"
        );
        self.expectations.push(expectation);
        self
    }

    fn mock(&self) -> &MockSigner {
        self.mock.get_or_init(|| {
            let mut mock = MockSigner::new();
            let mut occurrences: BTreeMap<&DerivationPath, usize> = BTreeMap::new();

            for expectation in &self.expectations {
                let occurrence = occurrences.entry(&expectation.derivation_path).or_default();
                let signatures = expectation.signatures(*occurrence);
                *occurrence += signatures.len();

                for signature in signatures {
                    let expected_path = expectation.derivation_path.clone();
                    mock.expect_sign()
                        .withf(move |_message, path| path == &expected_path)
                        .times(1)
                        .return_once(move |_message, _path| {
                            signature.map(|signature| signature.as_ref().to_vec())
                        });
                }
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
