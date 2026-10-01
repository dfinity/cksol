use crate::{
    address::{
        MinterPublicKeyNotYetAvailable, account_address, derive_public_key_from_account,
        fetch_and_record_minter_public_key, get_deposit_address, minter_address, minter_public_key,
    },
    state::{SchnorrPublicKey, event::EventType, read_state},
    test_fixtures::{
        EventsAssert, MINTER_ACCOUNT, account, init_schnorr_master_key, init_state,
        runtime::TestCanisterRuntime,
    },
};
use futures::join;
use ic_cdk_management_canister::SchnorrPublicKeyResult;
use ic_ed25519::{PocketIcMasterPublicKeyId, PublicKey};
use icrc_ledger_types::icrc1::account::Account;
use solana_address::Address;

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
}

mod fetch_and_record_minter_public_key_tests {
    use super::*;

    #[tokio::test]
    async fn records_the_fetched_key_then_skips_the_fetch() {
        init_state();
        let runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .with_schnorr_public_key(test_key_result());

        fetch_and_record_minter_public_key(&runtime).await;

        assert_eq!(
            read_state(|s| s.minter_public_key().cloned()),
            Some(test_key())
        );
        EventsAssert::from_recorded()
            .expect_event_eq(test_key_fetched_event())
            .assert_no_more_events();

        fetch_and_record_minter_public_key(&runtime).await;

        assert_eq!(runtime.schnorr_public_key_call_count(), 1);
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
            fetch_and_record_minter_public_key(&runtime),
            fetch_and_record_minter_public_key(&runtime)
        );

        assert_eq!(runtime.schnorr_public_key_call_count(), 2);
        assert_eq!(
            read_state(|s| s.minter_public_key().cloned()),
            Some(test_key())
        );
        EventsAssert::from_recorded()
            .expect_event_eq(test_key_fetched_event())
            .assert_no_more_events();
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
