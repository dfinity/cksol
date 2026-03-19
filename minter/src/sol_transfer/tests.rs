use super::*;
use crate::{
    state::read_state,
    test_fixtures::{init_schnorr_master_key, init_state},
};
use candid::Principal;
use ic_cdk::{call::CallRejected, management_canister::SignCallError};
use solana_address::Address;
use solana_signature::Signature;
use std::{cell::RefCell, collections::VecDeque};

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

fn setup() {
    init_state();
    init_schnorr_master_key();
}

fn derive_address(account: &Account) -> Address {
    let master_key = read_state(|s| s.minter_public_key().cloned().unwrap());
    Address::from(derive_public_key(&master_key, derivation_path(account)).serialize_raw())
}

#[tokio::test]
async fn should_create_signed_transaction_with_single_source() {
    setup();
    let source_account = Account {
        owner: Principal::from_slice(&[1, 2, 3]),
        subaccount: None,
    };
    let target_account = Account {
        owner: Principal::from_slice(&[4, 5, 6]),
        subaccount: None,
    };
    let amount: Lamport = 500_000_000;
    let blockhash = Hash::new_from_array([0xBB; 32]);
    let fake_signature = [0x42u8; 64];

    let source_address = derive_address(&source_account);
    let target_address = derive_address(&target_account);

    // Fee payer is the source, so only one signature needed
    let signer = MockSchnorrSigner::with_signatures(vec![fake_signature]);
    let (tx, signers) = create_signed_transfer_transaction(
        source_account,
        &[(source_account, amount)],
        target_account,
        blockhash,
        &signer,
    )
    .await
    .expect("transaction creation should succeed");

    // Verify signers list
    assert_eq!(signers, vec![source_account]);

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
async fn should_create_signed_transaction_with_multiple_sources() {
    setup();
    let account_1 = Account {
        owner: Principal::from_slice(&[1]),
        subaccount: None,
    };
    let account_2 = Account {
        owner: Principal::from_slice(&[2]),
        subaccount: None,
    };
    let target_account = Account {
        owner: Principal::from_slice(&[3]),
        subaccount: None,
    };
    let amount: Lamport = 100_000_000;
    let blockhash = Hash::new_from_array([0xDD; 32]);
    let fake_sig_1 = [0x11u8; 64];
    let fake_sig_2 = [0x22u8; 64];

    let source_1 = derive_address(&account_1);
    let source_2 = derive_address(&account_2);

    // Fee payer (account_1) signature first, then account_2
    let signer = MockSchnorrSigner::with_signatures(vec![fake_sig_1, fake_sig_2]);
    let (tx, signers) = create_signed_transfer_transaction(
        account_1,
        &[(account_1, amount), (account_2, amount)],
        target_account,
        blockhash,
        &signer,
    )
    .await
    .expect("transaction creation should succeed");

    // Verify signers list (fee payer first, then sources)
    assert_eq!(signers, vec![account_1, account_2]);

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
    setup();
    let source_account = Account {
        owner: Principal::from_slice(&[1]),
        subaccount: None,
    };
    let target_account = Account {
        owner: Principal::from_slice(&[2]),
        subaccount: None,
    };
    let blockhash = Hash::new_from_array([0xBB; 32]);

    let signer = MockSchnorrSigner::with_responses(vec![Err(SignCallError::CallFailed(
        CallRejected::with_rejection(4, "signing service unavailable".to_string()).into(),
    ))]);

    let result = create_signed_transfer_transaction(
        source_account,
        &[(source_account, 500_000_000)],
        target_account,
        blockhash,
        &signer,
    )
    .await;

    assert!(result.is_err());
}

#[tokio::test]
async fn should_fail_when_second_signing_fails() {
    setup();
    let account_1 = Account {
        owner: Principal::from_slice(&[1]),
        subaccount: None,
    };
    let account_2 = Account {
        owner: Principal::from_slice(&[2]),
        subaccount: None,
    };
    let target_account = Account {
        owner: Principal::from_slice(&[3]),
        subaccount: None,
    };
    let blockhash = Hash::new_from_array([0xDD; 32]);

    let signer = MockSchnorrSigner::with_responses(vec![
        Ok(vec![0x11; 64]),
        Err(SignCallError::CallFailed(
            CallRejected::with_rejection(5, "canister trapped".to_string()).into(),
        )),
    ]);

    let result = create_signed_transfer_transaction(
        account_1,
        &[(account_1, 100_000_000), (account_2, 100_000_000)],
        target_account,
        blockhash,
        &signer,
    )
    .await;

    assert!(result.is_err());
}

#[tokio::test]
async fn should_fail_when_too_many_signatures() {
    setup();
    let target_account = Account {
        owner: Principal::from_slice(&[0xFF]),
        subaccount: None,
    };
    let blockhash = Hash::new_from_array([0xBB; 32]);
    let signer = MockSchnorrSigner::with_signatures(vec![]);

    // Create MAX_SIGNATURES sources with a SEPARATE fee payer, resulting in MAX_SIGNATURES + 1
    // signatures
    let sources: Vec<(Account, Lamport)> = (0..MAX_SIGNATURES)
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

    // Fee payer is NOT in sources, so total signatures = sources + 1 = MAX_SIGNATURES + 1
    let fee_payer = Account {
        owner: Principal::from_slice(&[0xFE]),
        subaccount: None,
    };

    let result =
        create_signed_transfer_transaction(fee_payer, &sources, target_account, blockhash, &signer)
            .await;

    assert!(
        matches!(result, Err(CreateTransferError::TooManySignatures { max: MAX_SIGNATURES, got }) if got == MAX_SIGNATURES + 1)
    );
}

#[tokio::test]
async fn should_not_fail_for_max_signatures() {
    setup();
    let target_account = Account {
        owner: Principal::from_slice(&[0xFF]),
        subaccount: None,
    };
    let blockhash = Hash::new_from_array([0xBB; 32]);

    // Create MAX_SIGNATURES - 1 sources with a SEPARATE fee payer, resulting in exactly
    // MAX_SIGNATURES signatures
    let sources: Vec<(Account, Lamport)> = (0..MAX_SIGNATURES - 1)
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

    // Fee payer is NOT in sources, so total signatures = sources + 1 = MAX_SIGNATURES
    let fee_payer = Account {
        owner: Principal::from_slice(&[0xFE]),
        subaccount: None,
    };

    let signer = MockSchnorrSigner::with_signatures(vec![[0x11u8; 64]; MAX_SIGNATURES as usize]);

    let result =
        create_signed_transfer_transaction(fee_payer, &sources, target_account, blockhash, &signer)
            .await;

    assert!(result.is_ok());
}

#[tokio::test]
async fn should_create_signed_transaction_with_fee_payer() {
    setup();
    let fee_payer_account = Account {
        owner: Principal::from_slice(&[1]),
        subaccount: None,
    };
    let other_source = Account {
        owner: Principal::from_slice(&[2]),
        subaccount: None,
    };
    let target_account = Account {
        owner: Principal::from_slice(&[3]),
        subaccount: None,
    };
    let amount: Lamport = 100_000_000;
    let blockhash = Hash::new_from_array([0xAA; 32]);

    let fee_payer_address = derive_address(&fee_payer_account);
    let other_address = derive_address(&other_source);

    for sources in [
        // Fee payer *not* in sources
        vec![(other_source, amount)],
        // Fee payer in sources
        vec![(fee_payer_account, amount), (other_source, amount)],
    ] {
        // Two signatures needed in both cases (fee payer + other source)
        let signer = MockSchnorrSigner::with_signatures(vec![[0x11u8; 64], [0x22u8; 64]]);
        let (tx, signers) = create_signed_transfer_transaction(
            fee_payer_account,
            &sources,
            target_account,
            blockhash,
            &signer,
        )
        .await
        .expect("transaction creation should succeed");

        // Verify signers list (fee payer first, deduplicated)
        assert_eq!(signers, vec![fee_payer_account, other_source]);

        // Fee payer is always at position 0
        assert_eq!(tx.message.account_keys[0], fee_payer_address);

        // Two unique signers => two signatures
        assert_eq!(tx.signatures.len(), 2);

        assert_eq!(tx.message.instructions.len(), sources.len());

        // Verify all signers have non-default signatures
        let fee_payer_pos = tx
            .message
            .account_keys
            .iter()
            .position(|k| *k == fee_payer_address)
            .unwrap();
        let other_pos = tx
            .message
            .account_keys
            .iter()
            .position(|k| *k == other_address)
            .unwrap();
        assert_ne!(tx.signatures[fee_payer_pos], Signature::default());
        assert_ne!(tx.signatures[other_pos], Signature::default());
    }
}
