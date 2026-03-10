use super::*;
use ic_cdk::management_canister::{SignCallError, SignWithSchnorrArgs, SignWithSchnorrResult};
use ic_ed25519::{PocketIcMasterPublicKeyId, PublicKey};
use solana_address::Address;
use std::cell::RefCell;

struct MockSchnorrSigner {
    responses: RefCell<Vec<Result<SignWithSchnorrResult, SignCallError>>>,
}

impl MockSchnorrSigner {
    fn with_signatures(signatures: Vec<[u8; 64]>) -> Self {
        Self {
            responses: RefCell::new(
                signatures
                    .into_iter()
                    .map(|sig| {
                        Ok(SignWithSchnorrResult {
                            signature: sig.to_vec(),
                        })
                    })
                    .collect(),
            ),
        }
    }
}

impl SchnorrSigner for MockSchnorrSigner {
    async fn sign(
        &self,
        _args: &SignWithSchnorrArgs,
    ) -> Result<SignWithSchnorrResult, SignCallError> {
        self.responses
            .borrow_mut()
            .pop()
            .expect("MockSchnorrSigner: no more stub responses")
    }
}

fn test_master_key() -> SchnorrPublicKey {
    SchnorrPublicKey {
        public_key: PublicKey::pocketic_key(PocketIcMasterPublicKeyId::Key1),
        chain_code: [1; 32],
    }
}

#[test]
fn system_transfer_instruction_encoding() {
    let from = Address::from([1u8; 32]);
    let to = Address::from([2u8; 32]);
    let lamports: Lamport = 1_000_000_000;

    let ix = system_transfer_instruction(&from, &to, lamports);

    assert_eq!(ix.program_id, SYSTEM_PROGRAM_ID);
    assert_eq!(ix.accounts.len(), 2);
    assert_eq!(ix.accounts[0].pubkey, from);
    assert!(ix.accounts[0].is_signer);
    assert!(ix.accounts[0].is_writable);
    assert_eq!(ix.accounts[1].pubkey, to);
    assert!(!ix.accounts[1].is_signer);
    assert!(ix.accounts[1].is_writable);

    // Bincode: variant index 2 (u32 LE) + lamports (u64 LE)
    assert_eq!(ix.data.len(), 12);
    assert_eq!(&ix.data[..4], &2u32.to_le_bytes());
    assert_eq!(&ix.data[4..], &lamports.to_le_bytes());
}

#[test]
fn system_program_id_is_all_zeros() {
    assert_eq!(SYSTEM_PROGRAM_ID, Address::from([0u8; 32]));
    assert_eq!(
        SYSTEM_PROGRAM_ID.to_string(),
        "11111111111111111111111111111111"
    );
}

#[tokio::test]
async fn should_create_signed_transaction_single_source() {
    let master_key = test_master_key();
    let derivation_path = vec![vec![1u8], vec![2u8, 3u8]];
    let target_address = Address::from([0xAA; 32]);
    let amount: Lamport = 500_000_000;
    let blockhash = Hash::new_from_array([0xBB; 32]);
    let fake_signature = [0x42u8; 64];

    let source_address = Address::from(
        derive_public_key(&master_key, derivation_path.clone()).serialize_raw(),
    );

    let signer = MockSchnorrSigner::with_signatures(vec![fake_signature]);
    let tx = create_signed_transfer_transaction(
        &master_key,
        "test_key",
        &[derivation_path],
        target_address,
        amount,
        blockhash,
        &signer,
    )
    .await
    .expect("transaction creation should succeed");

    // Fee payer is the source address
    assert_eq!(tx.message.account_keys[0], source_address);
    // Target and system program are also in account keys
    assert!(tx.message.account_keys.contains(&target_address));
    assert!(tx.message.account_keys.contains(&SYSTEM_PROGRAM_ID));

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
    let derivation_path_1 = vec![vec![1u8]];
    let derivation_path_2 = vec![vec![2u8]];
    let target_address = Address::from([0xCC; 32]);
    let amount: Lamport = 100_000_000;
    let blockhash = Hash::new_from_array([0xDD; 32]);
    let fake_sig_1 = [0x11u8; 64];
    let fake_sig_2 = [0x22u8; 64];

    let source_1 = Address::from(
        derive_public_key(&master_key, derivation_path_1.clone()).serialize_raw(),
    );
    let source_2 = Address::from(
        derive_public_key(&master_key, derivation_path_2.clone()).serialize_raw(),
    );

    // MockSchnorrSigner pops from the back, so push in reverse order.
    let signer = MockSchnorrSigner::with_signatures(vec![fake_sig_2, fake_sig_1]);
    let tx = create_signed_transfer_transaction(
        &master_key,
        "test_key",
        &[derivation_path_1, derivation_path_2],
        target_address,
        amount,
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
