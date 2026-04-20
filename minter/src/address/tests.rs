mod derive_key {
    use candid::Principal;
    use ic_ed25519::{PocketIcMasterPublicKeyId, PublicKey};
    use icrc_ledger_types::icrc1::account::Account;

    use crate::{address::derive_public_key_from_account, state::SchnorrPublicKey};

    #[test]
    fn test_derive_default_subaccount() {
        let public_key = PublicKey::pocketic_key(PocketIcMasterPublicKeyId::DfxTestKey);
        let master_key = SchnorrPublicKey {
            public_key,
            chain_code: [1; 32],
        };
        let account_none = Account {
            owner: Principal::from_slice(&[1]),
            subaccount: None,
        };
        let account_zeros = Account {
            owner: Principal::from_slice(&[1]),
            subaccount: Some([0; 32]),
        };
        assert_eq!(
            derive_public_key_from_account(&master_key, &account_none),
            derive_public_key_from_account(&master_key, &account_zeros)
        );
    }

    #[test]
    fn test_derive_different_principal() {
        let public_key = PublicKey::pocketic_key(PocketIcMasterPublicKeyId::DfxTestKey);
        let master_key = SchnorrPublicKey {
            public_key,
            chain_code: [1; 32],
        };
        let account1 = Account {
            owner: Principal::from_slice(&[1]),
            subaccount: None,
        };
        let account2 = Account {
            owner: Principal::from_slice(&[2]),
            subaccount: None,
        };
        assert_ne!(
            derive_public_key_from_account(&master_key, &account1),
            derive_public_key_from_account(&master_key, &account2)
        );
    }

    #[test]
    fn test_derive_different_subaccount() {
        let public_key = PublicKey::pocketic_key(PocketIcMasterPublicKeyId::DfxTestKey);
        let master_key = SchnorrPublicKey {
            public_key,
            chain_code: [1; 32],
        };
        let account1 = Account {
            owner: Principal::from_slice(&[1]),
            subaccount: Some([10; 32]),
        };
        let account2 = Account {
            owner: Principal::from_slice(&[1]),
            subaccount: Some([11; 32]),
        };
        assert_ne!(
            derive_public_key_from_account(&master_key, &account1),
            derive_public_key_from_account(&master_key, &account2)
        );
    }

    #[test]
    fn test_derive_different_chain_code() {
        let public_key = PublicKey::pocketic_key(PocketIcMasterPublicKeyId::DfxTestKey);
        let master_key1 = SchnorrPublicKey {
            public_key,
            chain_code: [1; 32],
        };
        let master_key2 = SchnorrPublicKey {
            public_key,
            chain_code: [2; 32],
        };
        let account = Account {
            owner: Principal::from_slice(&[1]),
            subaccount: Some([10; 32]),
        };
        assert_ne!(
            derive_public_key_from_account(&master_key1, &account),
            derive_public_key_from_account(&master_key2, &account)
        );
    }
}

mod lazy_schnorr_master_key {
    use ic_cdk_management_canister::SchnorrPublicKeyResult;
    use ic_ed25519::{PocketIcMasterPublicKeyId, PublicKey};

    use crate::{
        address::lazy_get_schnorr_master_key,
        state::{SchnorrPublicKey, read_state, reset_state},
        test_fixtures::{init_schnorr_master_key, init_state, runtime::TestCanisterRuntime},
    };

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

    #[tokio::test]
    async fn uses_cached_key_without_calling_runtime() {
        init_state();
        init_schnorr_master_key();
        let cached = read_state(|s| s.minter_public_key().cloned().unwrap());

        // No stub registered — panics if schnorr_public_key is called on the runtime.
        let runtime = TestCanisterRuntime::new();
        let result = lazy_get_schnorr_master_key(&runtime).await;

        assert_eq!(result, cached);
        reset_state();
    }

    #[tokio::test]
    async fn fetches_key_and_caches_it() {
        init_state();
        let runtime = TestCanisterRuntime::new().with_schnorr_public_key(test_key_result());

        let result = lazy_get_schnorr_master_key(&runtime).await;

        assert_eq!(result, test_key());
        // Second call must use the cache — no more stubs, so it would panic if it hit the runtime.
        let cached = lazy_get_schnorr_master_key(&runtime).await;
        assert_eq!(result, cached);
        reset_state();
    }
}
