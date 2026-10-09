use crate::{
    address::{
        MINTER_DERIVATION_PATH, MinterPublicKeyNotYetAvailable, account_address,
        derive_public_key_from_account, fetch_and_record_minter_public_key, get_deposit_address,
        minter_address, minter_public_key,
    },
    state::{SchnorrPublicKey, event::EventType, read_state},
    test_fixtures::{
        EventsAssert, MINTER_ACCOUNT, account, init_schnorr_master_key, init_state,
        runtime::TestCanisterRuntime,
    },
};
use futures::{FutureExt, join};
use ic_cdk_management_canister::SchnorrPublicKeyResult;
use ic_ed25519::{CanisterId, MasterPublicKeyId, PocketIcMasterPublicKeyId, PublicKey};
use icrc_ledger_types::icrc1::account::Account;
use solana_address::Address;
use std::panic::AssertUnwindSafe;

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

mod minter_address_tests {
    use super::*;
    use solana_address::address;

    #[test]
    fn should_differ_from_deposit_address_of_minter_account() {
        assert_ne!(
            minter_address(&test_key()),
            account_address(&test_key(), &MINTER_ACCOUNT)
        );
    }

    #[test]
    fn should_be_raw_master_public_key() {
        let master_key = test_key();
        assert_eq!(
            minter_address(&master_key),
            Address::from(master_key.public_key.serialize_raw())
        );
    }

    #[test]
    fn should_derive_mainnet_minter_addresses_offline() {
        const CKSOL_MINTER_PRODUCTION_CANISTER_ID: &str = "lh22c-kyaaa-aaaar-qb5nq-cai";
        const CKSOL_MINTER_STAGING_CANISTER_ID: &str = "ljyxk-riaaa-aaaar-qb5mq-cai";

        for (minter_id, expected_address) in [
            (
                CKSOL_MINTER_PRODUCTION_CANISTER_ID,
                address!("GXVewvv6HehcLFYCmwpqh5CrMjqN9zJ8rqzGq3HEt9Ax"),
            ),
            (
                CKSOL_MINTER_STAGING_CANISTER_ID,
                address!("Br8eRkeya8hy3sHCYqtWqGeNNZ349aPSUKGVYFWKKer1"),
            ),
        ] {
            let (master_public_key, chain_code) = PublicKey::derive_mainnet_key(
                MasterPublicKeyId::Key1,
                &CanisterId::from_text(minter_id).unwrap(),
                &MINTER_DERIVATION_PATH,
            );
            let minter_address = minter_address(&SchnorrPublicKey {
                public_key: master_public_key,
                chain_code,
            });
            assert_eq!(
                minter_address, expected_address,
                "unexpected main address for minter {minter_id}"
            );
        }
    }
}

mod fetch_and_record_minter_public_key_tests {
    use super::*;

    #[tokio::test]
    async fn records_the_fetched_key_then_skips_the_fetch() {
        init_state();
        let runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .with_schnorr_public_key(test_key_result());

        fetch_and_record_minter_public_key(runtime.clone()).await;

        assert_eq!(
            read_state(|s| s.minter_public_key().cloned()),
            Some(test_key())
        );
        EventsAssert::from_recorded()
            .expect_event_eq(test_key_fetched_event())
            .assert_no_more_events();

        fetch_and_record_minter_public_key(runtime.clone()).await;

        assert_eq!(runtime.schnorr_public_key_call_count(), 1);
        assert_eq!(runtime.set_timer_call_count(), 0);
        EventsAssert::from_recorded()
            .expect_event_eq(test_key_fetched_event())
            .assert_no_more_events();
    }

    #[tokio::test]
    async fn interleaved_first_calls_both_fetch_but_record_one_event() {
        init_state();
        let runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .with_schnorr_public_key(test_key_result())
            .with_schnorr_public_key(test_key_result());

        join!(
            fetch_and_record_minter_public_key(runtime.clone()),
            fetch_and_record_minter_public_key(runtime.clone())
        );

        assert_eq!(runtime.schnorr_public_key_call_count(), 2);
        assert_eq!(runtime.set_timer_call_count(), 0);
        assert_eq!(
            read_state(|s| s.minter_public_key().cloned()),
            Some(test_key())
        );
        EventsAssert::from_recorded()
            .expect_event_eq(test_key_fetched_event())
            .assert_no_more_events();
    }

    #[tokio::test]
    async fn reschedules_the_fetch_after_a_failed_call() {
        init_state();
        let runtime = TestCanisterRuntime::new().with_schnorr_public_key_call_failure();

        fetch_and_record_minter_public_key(runtime.clone()).await;

        assert_eq!(runtime.set_timer_call_count(), 1);
        assert_eq!(read_state(|s| s.minter_public_key().cloned()), None);
        EventsAssert::from_recorded().assert_no_more_events();
    }

    #[tokio::test]
    async fn reschedules_the_fetch_after_a_trap() {
        init_state();
        let runtime = TestCanisterRuntime::new();

        let fetch = AssertUnwindSafe(fetch_and_record_minter_public_key(runtime.clone()))
            .catch_unwind()
            .await;

        assert!(fetch.is_err());
        assert_eq!(runtime.set_timer_call_count(), 1);
        assert_eq!(read_state(|s| s.minter_public_key().cloned()), None);
        EventsAssert::from_recorded().assert_no_more_events();
    }

    fn test_key_fetched_event() -> EventType {
        let key = test_key();
        EventType::MinterPublicKeyFetched {
            public_key: key.public_key,
            chain_code: key.chain_code,
        }
    }
}

mod minter_public_key_tests {
    use super::*;

    #[test]
    fn errors_until_the_key_is_available() {
        init_state();

        assert_eq!(minter_public_key(), Err(MinterPublicKeyNotYetAvailable));

        init_schnorr_master_key();

        assert_eq!(
            minter_public_key(),
            Ok(read_state(|s| s.minter_public_key().cloned().unwrap()))
        );
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
