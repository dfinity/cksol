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
/// account are both readable in the signature bytes: the owner first, the subaccount after
/// the widest principal, and how far the owner falls short of that width last.
///
/// Recording the shortfall rather than the owner length keeps the mapping injective — the
/// owners `[1]` and `[1, 0]` would otherwise share a signature — while leaving the byte zero
/// for a full-width owner, so `account_signature(&account(i)) == signature(i)` holds.
pub(super) fn derivation_path_signature(derivation_path: &DerivationPath) -> Signature {
    const MAX_PRINCIPAL_LEN: usize = 29;
    const SUBACCOUNT_OFFSET: usize = MAX_PRINCIPAL_LEN;
    const OWNER_SHORTFALL_OFFSET: usize = SUBACCOUNT_OFFSET + 32;

    let [_schema_version, owner, subaccount] = derivation_path.as_slice() else {
        panic!("BUG: unexpected derivation path {derivation_path:?}");
    };
    let owner_shortfall = MAX_PRINCIPAL_LEN
        .checked_sub(owner.len())
        .expect("BUG: principal wider than a derivation path can hold");

    let mut bytes = [0_u8; 64];
    bytes[..owner.len()].copy_from_slice(owner);
    bytes[SUBACCOUNT_OFFSET..SUBACCOUNT_OFFSET + subaccount.len()].copy_from_slice(subaccount);
    bytes[OWNER_SHORTFALL_OFFSET] = owner_shortfall as u8;
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
/// Tests that need a different answer register it up front with [`Self::add_signature`].
/// Overrides are consumed in registration order and must all be used, so an account
/// registered twice signs twice.
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
