use crate::{
    address::{
        account_address, derive_public_key_from_account, get_deposit_address,
        lazy_get_schnorr_master_key,
    },
    state::{SchnorrPublicKey, read_state},
    test_fixtures::{account, init_schnorr_master_key, init_state, runtime::TestCanisterRuntime},
};
use ic_cdk_management_canister::SchnorrPublicKeyResult;
use ic_ed25519::{PocketIcMasterPublicKeyId, PublicKey};
use icrc_ledger_types::icrc1::account::Account;

#[test]
fn test_derive_default_subaccount() {
    let account_none = account(1);
    let account_zeros = Account {
        subaccount: Some([0; 32]),
        ..account(1)
    };
    assert_eq!(
        derive_public_key_from_account(&test_key(), &account_none),
        derive_public_key_from_account(&test_key(), &account_zeros)
    );
}

#[test]
fn test_derive_different_principal() {
    assert_ne!(
        derive_public_key_from_account(&test_key(), &account(1)),
        derive_public_key_from_account(&test_key(), &account(2))
    );
}

#[test]
fn test_derive_different_subaccount() {
    let account1 = Account {
        subaccount: Some([10; 32]),
        ..account(1)
    };
    let account2 = Account {
        subaccount: Some([11; 32]),
        ..account(1)
    };
    assert_ne!(
        derive_public_key_from_account(&test_key(), &account1),
        derive_public_key_from_account(&test_key(), &account2)
    );
}

#[test]
fn test_derive_different_chain_code() {
    let master_key2 = SchnorrPublicKey {
        chain_code: [2; 32],
        ..test_key()
    };
    let acc = Account {
        subaccount: Some([10; 32]),
        ..account(1)
    };
    assert_ne!(
        derive_public_key_from_account(&test_key(), &acc),
        derive_public_key_from_account(&master_key2, &acc)
    );
}

mod lazy_schnorr_master_key {
    use super::*;

    #[tokio::test]
    async fn fetches_key_then_uses_cache() {
        init_state();

        // First call: no key cached, stub returns test_key.
        let runtime = TestCanisterRuntime::new().with_schnorr_public_key(test_key_result());
        let result = lazy_get_schnorr_master_key(&runtime).await;
        assert_eq!(result, test_key());

        // Second call: key is now cached — no stubs left, would panic if it hit the runtime.
        let cached = lazy_get_schnorr_master_key(&runtime).await;
        assert_eq!(result, cached);
    }
}

mod get_deposit_address_tests {
    use super::*;

    #[test]
    fn returns_address_when_key_is_cached() {
        init_state();
        init_schnorr_master_key();
        let master_key = read_state(|s| s.minter_public_key().cloned().unwrap());
        let acc = account(1);

        assert_eq!(
            get_deposit_address(&acc),
            account_address(&master_key, &acc),
        );
    }

    #[test]
    #[should_panic]
    fn traps_when_key_is_not_cached() {
        init_state();
        get_deposit_address(&account(1));
    }
}

fn test_key() -> SchnorrPublicKey {
    SchnorrPublicKey {
        public_key: PublicKey::pocketic_key(PocketIcMasterPublicKeyId::DfxTestKey),
        chain_code: [42; 32],
    }
}

fn test_key_result() -> SchnorrPublicKeyResult {
    let key = test_key();
    SchnorrPublicKeyResult {
        public_key: key.public_key.serialize_raw().to_vec(),
        chain_code: key.chain_code.to_vec(),
    }
}
