use super::*;
use crate::{
    state::read_state,
    test_fixtures::{
        init_schnorr_master_key, init_state, runtime::TestCanisterRuntime,
        signer::MockSchnorrSigner,
    },
};
use assert_matches::assert_matches;
use candid::Principal;
use ic_cdk::call::CallRejected;
use ic_cdk_management_canister::SignCallError;
use solana_address::Address;
use solana_signature::Signature;

fn setup() {
    init_state();
    init_schnorr_master_key();
}

fn derive_address(account: &Account) -> Address {
    let master_key = read_state(|s| s.minter_public_key().cloned().unwrap());
    Address::from(derive_public_key(&master_key, derivation_path(account)).serialize_raw())
}

mod batch_consolidation_tests {
    use super::*;

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
        let signature = [0x42u8; 64];

        let source_address = derive_address(&source_account);
        let target_address = derive_address(&target_account);

        let signer = MockSchnorrSigner::with_signatures(vec![signature]);
        let (tx, signers) = create_signed_consolidation_transaction(
            vec![(source_account, amount)],
            target_address,
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
        assert_eq!(tx.signatures[0], Signature::from(signature));

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
        let sig_1 = [0x11u8; 64];
        let sig_2 = [0x22u8; 64];

        let source_1 = derive_address(&account_1);
        let source_2 = derive_address(&account_2);

        // Fee payer (account_1) signature first, then account_2
        let signer = MockSchnorrSigner::with_signatures(vec![sig_1, sig_2]);
        let (tx, signers) = create_signed_consolidation_transaction(
            vec![(account_1, amount), (account_2, amount)],
            derive_address(&target_account),
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
        assert_eq!(tx.signatures[pos_1], Signature::from(sig_1));
        assert_eq!(tx.signatures[pos_2], Signature::from(sig_2));
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

        let result = create_signed_consolidation_transaction(
            vec![(source_account, 500_000_000)],
            derive_address(&target_account),
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

        let result = create_signed_consolidation_transaction(
            vec![(account_1, 100_000_000), (account_2, 100_000_000)],
            derive_address(&target_account),
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
        let signer = MockSchnorrSigner::with_signatures([[0xAA; 64]; MAX_SIGNATURES as usize + 1]);

        // Create MAX_SIGNATURES + 1 unique sources, exceeding the limit
        let sources: Vec<(Account, Lamport)> = (0..=MAX_SIGNATURES)
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

        let result = create_signed_consolidation_transaction(
            sources,
            derive_address(&target_account),
            blockhash,
            &signer,
        )
        .await;

        assert_matches!(
            result,
            Err(CreateTransferError::TransactionTooLarge {
                max: MAX_TX_SIZE,
                ..
            })
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

        // Create exactly MAX_SIGNATURES unique sources
        let sources: Vec<(Account, Lamport)> = (0..MAX_SIGNATURES)
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

        let signer =
            MockSchnorrSigner::with_signatures(vec![[0x11u8; 64]; MAX_SIGNATURES as usize]);

        let result = create_signed_consolidation_transaction(
            sources,
            derive_address(&target_account),
            blockhash,
            &signer,
        )
        .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn should_fail_when_transaction_too_large() {
        setup();
        let target_account = Account {
            owner: Principal::from_slice(&[0xFF]),
            subaccount: None,
        };
        let blockhash = Hash::new_from_array([0xBB; 32]);
        // MAX_SIGNATURES + 1 unique sources to exceed MAX_TX_SIZE
        const NUM_SOURCES: usize = MAX_SIGNATURES as usize + 1;
        let signer = MockSchnorrSigner::with_signatures([[0xAA; 64]; NUM_SOURCES]);

        let sources: Vec<(Account, Lamport)> = (0..NUM_SOURCES)
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

        let result = create_signed_consolidation_transaction(
            sources,
            derive_address(&target_account),
            blockhash,
            &signer,
        )
        .await;

        assert_matches!(
            result,
            Err(CreateTransferError::TransactionTooLarge {
                max: MAX_TX_SIZE,
                ..
            })
        );
    }

    #[tokio::test]
    async fn should_reduce_duplicate_accounts() {
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
        let blockhash = Hash::new_from_array([0xAA; 32]);

        // Two entries for account_1 should be reduced to one transfer
        let signer = MockSchnorrSigner::with_signatures(vec![[0x11u8; 64], [0x22u8; 64]]);
        let (tx, signers) = create_signed_consolidation_transaction(
            vec![
                (account_1, 100_000_000),
                (account_2, 200_000_000),
                (account_1, 300_000_000),
            ],
            derive_address(&target_account),
            blockhash,
            &signer,
        )
        .await
        .expect("transaction creation should succeed");

        // Two unique signers
        assert_eq!(signers, vec![account_1, account_2]);
        assert_eq!(tx.signatures.len(), 2);
        // Two transfer instructions (one per unique account)
        assert_eq!(tx.message.instructions.len(), 2);
    }

    #[tokio::test]
    async fn should_use_first_source_as_fee_payer() {
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
        let blockhash = Hash::new_from_array([0xAA; 32]);

        let account_1_address = derive_address(&account_1);

        let signer = MockSchnorrSigner::with_signatures(vec![[0x11u8; 64], [0x22u8; 64]]);
        let (tx, _signers) = create_signed_consolidation_transaction(
            vec![(account_1, 100_000_000), (account_2, 200_000_000)],
            derive_address(&target_account),
            blockhash,
            &signer,
        )
        .await
        .expect("transaction creation should succeed");

        // First source (account_1) is the fee payer (position 0)
        assert_eq!(tx.message.account_keys[0], account_1_address);
    }
}

mod batch_withdrawal_tests {
    use super::*;
    use crate::test_fixtures::runtime::TEST_CANISTER_ID;

    fn minter_account() -> Account {
        Account::from(TEST_CANISTER_ID)
    }

    fn minter_address() -> Address {
        derive_address(&minter_account())
    }

    #[tokio::test]
    async fn should_create_batch_withdrawal_with_single_target() {
        setup();
        let target = Address::new_from_array([0xAA; 32]);
        let amount: Lamport = 500_000_000;
        let blockhash = Hash::new_from_array([0xBB; 32]);
        let sig = [0x42u8; 64];

        let runtime = TestCanisterRuntime::new().add_signature(sig);
        let (tx, signers) =
            create_signed_batch_withdrawal_transaction(&runtime, &[(target, amount)], blockhash)
                .await
                .expect("transaction creation should succeed");

        assert_eq!(signers, vec![minter_account()]);
        assert_eq!(tx.signatures.len(), 1);
        assert_eq!(tx.signatures[0], Signature::from(sig));
        assert_eq!(tx.message.account_keys[0], minter_address());
        assert!(tx.message.account_keys.contains(&target));
        assert_eq!(tx.message.instructions.len(), 1);
        assert_eq!(tx.message.recent_blockhash, blockhash);
    }

    #[tokio::test]
    async fn should_create_batch_withdrawal_with_multiple_targets() {
        setup();
        let target_1 = Address::new_from_array([0xAA; 32]);
        let target_2 = Address::new_from_array([0xBB; 32]);
        let target_3 = Address::new_from_array([0xCC; 32]);
        let blockhash = Hash::new_from_array([0xDD; 32]);
        let sig = [0x11u8; 64];

        let runtime = TestCanisterRuntime::new().add_signature(sig);
        let (tx, signers) = create_signed_batch_withdrawal_transaction(
            &runtime,
            &[(target_1, 100), (target_2, 200), (target_3, 300)],
            blockhash,
        )
        .await
        .expect("transaction creation should succeed");

        // Only the minter signs
        assert_eq!(signers, vec![minter_account()]);
        assert_eq!(tx.signatures.len(), 1);

        // Fee payer is at position 0
        assert_eq!(tx.message.account_keys[0], minter_address());

        // All targets are in account keys
        assert!(tx.message.account_keys.contains(&target_1));
        assert!(tx.message.account_keys.contains(&target_2));
        assert!(tx.message.account_keys.contains(&target_3));

        // One instruction per target
        assert_eq!(tx.message.instructions.len(), 3);
    }

    #[tokio::test]
    async fn should_fail_when_signing_fails() {
        setup();
        let target = Address::new_from_array([0xAA; 32]);
        let blockhash = Hash::new_from_array([0xBB; 32]);

        let runtime =
            TestCanisterRuntime::new().add_schnorr_signing_error(SignCallError::CallFailed(
                CallRejected::with_rejection(4, "signing service unavailable".to_string()).into(),
            ));

        let result =
            create_signed_batch_withdrawal_transaction(&runtime, &[(target, 100)], blockhash).await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn should_create_batch_withdrawal_at_max_capacity() {
        setup();
        let blockhash = Hash::new_from_array([0xDD; 32]);
        let sig = [0x42u8; 64];

        let targets: Vec<(Address, Lamport)> = (0..MAX_WITHDRAWALS_PER_TX)
            .map(|i| {
                let mut addr = [0u8; 32];
                addr[0] = i as u8;
                addr[1] = (i >> 8) as u8;
                (Address::new_from_array(addr), 1_000_000)
            })
            .collect();

        let runtime = TestCanisterRuntime::new().add_signature(sig);
        let (tx, signers) =
            create_signed_batch_withdrawal_transaction(&runtime, &targets, blockhash)
                .await
                .expect("transaction creation should succeed at max capacity");

        assert_eq!(signers, vec![minter_account()]);
        assert_eq!(tx.signatures.len(), 1);
        assert_eq!(tx.message.instructions.len(), MAX_WITHDRAWALS_PER_TX);
    }

    #[tokio::test]
    async fn should_return_error_when_exceeding_tx_size_limit() {
        setup();
        let blockhash = Hash::new_from_array([0xDD; 32]);

        // Each additional target adds ~49 bytes (32-byte key + 17-byte instruction).
        // With a base of ~166 bytes and MAX_TX_SIZE = 1232, the limit is around 21-22.
        // Use 25 targets to reliably exceed the limit.
        const NUM_TARGETS: usize = 25;
        let targets: Vec<(Address, Lamport)> = (0..NUM_TARGETS)
            .map(|i| {
                let mut addr = [0u8; 32];
                addr[0] = i as u8;
                (Address::new_from_array(addr), 1_000_000)
            })
            .collect();

        let mut runtime = TestCanisterRuntime::new();
        for _ in 0..NUM_TARGETS {
            runtime = runtime.add_signature([0xAA; 64]);
        }

        let result =
            create_signed_batch_withdrawal_transaction(&runtime, &targets, blockhash).await;

        assert_matches!(
            result,
            Err(CreateTransferError::TransactionTooLarge {
                max: MAX_TX_SIZE,
                ..
            })
        );
    }
}
