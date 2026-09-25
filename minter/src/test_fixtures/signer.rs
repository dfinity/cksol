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

/// The [`Signature`] the mock signer returns the `occurrence`-th time `derivation_path`
/// signs, as the hash of both. Hashing every component under its own length keeps the
/// mapping injective, so no two accounts and no two signatures by the same account collide.
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
/// account signed it and from how many times that account has signed before.
///
/// Tests that need a different answer register it up front with [`Self::add_signature`].
/// Overrides are consumed in registration order and must all be used, so an account
/// registered twice signs twice; they do not advance the occurrence count.
#[derive(Clone, Default)]
pub struct MockSchnorrSigner {
    overrides: Vec<(DerivationPath, Result<Vec<u8>, SignCallError>)>,
    mock: Arc<OnceLock<MockSigner>>,
}

impl MockSchnorrSigner {
    pub fn add_signature(
        mut self,
        account: &Account,
        signature: Result<Signature, SignCallError>,
    ) -> Self {
        assert!(
            self.mock.get().is_none(),
            "BUG: register all signing overrides before the first signing request"
        );
        self.overrides.push((
            derivation_path(account),
            signature.map(|signature| signature.as_ref().to_vec()),
        ));
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

            let mut occurrences: BTreeMap<DerivationPath, usize> = BTreeMap::new();
            mock.expect_sign().returning(move |_message, path| {
                let occurrence = occurrences.entry(path.clone()).or_default();
                let signature = derivation_path_signature(&path, *occurrence);
                *occurrence += 1;
                Ok(signature.as_ref().to_vec())
            });
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
