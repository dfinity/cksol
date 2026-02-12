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
