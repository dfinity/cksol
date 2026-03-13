use super::*;
use candid::Principal;
use ic_cdk::call::CallRejected;
use ic_cdk::management_canister::SignCallError;
use ic_ed25519::{PocketIcMasterPublicKeyId, PublicKey};
use solana_address::Address;
use std::cell::RefCell;
use std::collections::VecDeque;

struct MockSchnorrSigner {
    responses: RefCell<VecDeque<Result<Vec<u8>, SignCallError>>>,
}

impl MockSchnorrSigner {
    fn with_signatures(signatures: Vec<[u8; 64]>) -> Self {
        Self {
            responses: RefCell::new(signatures.into_iter().map(|sig| Ok(sig.to_vec())).collect()),
        }
    }

    fn with_responses(responses: Vec<Result<Vec<u8>, SignCallError>>) -> Self {
        Self {
            responses: RefCell::new(responses.into()),
        }
    }
}

impl SchnorrSigner for MockSchnorrSigner {
    async fn sign(
        &self,
        _message: Vec<u8>,
        _derivation_path: DerivationPath,
    ) -> Result<Vec<u8>, SignCallError> {
        self.responses
            .borrow_mut()
            .pop_front()
            .expect("MockSchnorrSigner: no more stub responses")
    }
}

fn test_master_key() -> SchnorrPublicKey {
    SchnorrPublicKey {
        public_key: PublicKey::pocketic_key(PocketIcMasterPublicKeyId::Key1),
        chain_code: [1; 32],
    }
}

#[tokio::test]
async fn should_create_signed_transaction_single_source() {
    let master_key = test_master_key();
    let account = Account {
        owner: Principal::from_slice(&[1, 2, 3]),
        subaccount: None,
    };
    let target_address = Address::from([0xAA; 32]);
    let amount: Lamport = 500_000_000;
    let blockhash = Hash::new_from_array([0xBB; 32]);
    let fake_signature = [0x42u8; 64];

    let source_address =
        Address::from(derive_public_key(&master_key, derivation_path(&account)).serialize_raw());

    let signer = MockSchnorrSigner::with_signatures(vec![fake_signature]);
    let tx = create_signed_transfer_transaction(
        &master_key,
        &[(account, amount)],
        target_address,
        blockhash,
        &signer,
    )
    .await
    .expect("transaction creation should succeed");

    // Fee payer is the source address
    assert_eq!(tx.message.account_keys[0], source_address);
    // Target and system program are also in account keys
    assert!(tx.message.account_keys.contains(&target_address));
    // Should contain system program id
    assert!(
        tx.message
            .account_keys
            .contains(&Address::new_from_array([0u8; 32]))
    );

    // One transfer instruction
    assert_eq!(tx.message.instructions.len(), 1);

    // Signature is placed for the source address (position 0 = fee payer)
    assert_eq!(tx.signatures[0], Signature::from(fake_signature));

    // Recent blockhash is set
    assert_eq!(tx.message.recent_blockhash, blockhash);
}

#[tokio::test]
async fn should_create_signed_transaction_multiple_sources() {
    let master_key = test_master_key();
    let account_1 = Account {
        owner: Principal::from_slice(&[1]),
        subaccount: None,
    };
    let account_2 = Account {
        owner: Principal::from_slice(&[2]),
        subaccount: None,
    };
    let target_address = Address::from([0xCC; 32]);
    let amount: Lamport = 100_000_000;
    let blockhash = Hash::new_from_array([0xDD; 32]);
    let fake_sig_1 = [0x11u8; 64];
    let fake_sig_2 = [0x22u8; 64];

    let source_1 =
        Address::from(derive_public_key(&master_key, derivation_path(&account_1)).serialize_raw());
    let source_2 =
        Address::from(derive_public_key(&master_key, derivation_path(&account_2)).serialize_raw());
    let signer = MockSchnorrSigner::with_signatures(vec![fake_sig_1, fake_sig_2]);
    let tx = create_signed_transfer_transaction(
        &master_key,
        &[(account_1, amount), (account_2, amount)],
        target_address,
        blockhash,
        &signer,
    )
    .await
    .expect("transaction creation should succeed");

    // Two signers => two signatures
    assert_eq!(tx.signatures.len(), 2);
    // Fee payer is source_1
    assert_eq!(tx.message.account_keys[0], source_1);

    // Two transfer instructions
    assert_eq!(tx.message.instructions.len(), 2);

    // Verify signatures are at correct positions
    let pos_1 = tx
        .message
        .account_keys
        .iter()
        .position(|k| *k == source_1)
        .unwrap();
    let pos_2 = tx
        .message
        .account_keys
        .iter()
        .position(|k| *k == source_2)
        .unwrap();
    assert_eq!(tx.signatures[pos_1], Signature::from(fake_sig_1));
    assert_eq!(tx.signatures[pos_2], Signature::from(fake_sig_2));
}

#[tokio::test]
async fn should_fail_when_signing_is_rejected() {
    let master_key = test_master_key();
    let account = Account {
        owner: Principal::from_slice(&[1]),
        subaccount: None,
    };
    let target_address = Address::from([0xAA; 32]);
    let blockhash = Hash::new_from_array([0xBB; 32]);

    let signer = MockSchnorrSigner::with_responses(vec![Err(SignCallError::CallFailed(
        CallRejected::with_rejection(4, "signing service unavailable".to_string()).into(),
    ))]);

    let result = create_signed_transfer_transaction(
        &master_key,
        &[(account, 500_000_000)],
        target_address,
        blockhash,
        &signer,
    )
    .await;

    assert!(result.is_err());
}

#[tokio::test]
async fn should_fail_when_second_signing_fails() {
    let master_key = test_master_key();
    let account_1 = Account {
        owner: Principal::from_slice(&[1]),
        subaccount: None,
    };
    let account_2 = Account {
        owner: Principal::from_slice(&[2]),
        subaccount: None,
    };
    let target_address = Address::from([0xCC; 32]);
    let blockhash = Hash::new_from_array([0xDD; 32]);

    let signer = MockSchnorrSigner::with_responses(vec![
        Ok(vec![0x11; 64]),
        Err(SignCallError::CallFailed(
            CallRejected::with_rejection(5, "canister trapped".to_string()).into(),
        )),
    ]);

    let result = create_signed_transfer_transaction(
        &master_key,
        &[(account_1, 100_000_000), (account_2, 100_000_000)],
        target_address,
        blockhash,
        &signer,
    )
    .await;

    assert!(result.is_err());
}

#[tokio::test]
async fn should_fail_when_too_many_sources() {
    let master_key = test_master_key();
    let target_address = Address::from([0xAA; 32]);
    let blockhash = Hash::new_from_array([0xBB; 32]);
    let signer = MockSchnorrSigner::with_signatures(vec![]);

    let sources: Vec<(Account, Lamport)> = (0..MAX_SOURCES + 1)
        .map(|i| {
            (
                Account {
                    owner: Principal::from_slice(&[i as u8]),
                    subaccount: None,
                },
                100_000_000,
            )
        })
        .collect();

    let result = create_signed_transfer_transaction(
        &master_key,
        &sources,
        target_address,
        blockhash,
        &signer,
    )
    .await;

    assert!(
        matches!(result, Err(CreateTransferError::TooManySources { max: MAX_SOURCES, got }) if got == MAX_SOURCES + 1)
    );
}

#[tokio::test]
async fn should_no_fail_for_max_sources() {
    let master_key = test_master_key();
    let target_address = Address::from([0xAA; 32]);
    let blockhash = Hash::new_from_array([0xBB; 32]);

    let sources: Vec<(Account, Lamport)> = (0..MAX_SOURCES)
        .map(|i| {
            (
                Account {
                    owner: Principal::from_slice(&[i as u8 + 1; 29]),
                    subaccount: Some([3u8; 32]),
                },
                u64::MAX,
            )
        })
        .collect();

    let signer = MockSchnorrSigner::with_signatures(vec![[0x11u8; 64]; MAX_SOURCES as usize]);

    let result = create_signed_transfer_transaction(
        &master_key,
        &sources,
        target_address,
        blockhash,
        &signer,
    )
    .await;

    assert!(result.is_ok());
}
