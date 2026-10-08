use super::*;
use crate::test_fixtures::signer::{sign_as_minter, sign_for};
use crate::{
    address::{derivation_path, derive_public_key},
    constants::FEE_PER_SIGNATURE,
    state::{event::VersionedMessage, read_state},
    test_fixtures::{
        MINTER_ADDRESS, account_signature, init_schnorr_master_key, init_state, minter_signature,
        runtime::TestCanisterRuntime,
    },
};
use assert_matches::assert_matches;
use candid::Principal;
use ic_cdk::call::CallRejected;
use ic_cdk_management_canister::SignCallError;
use solana_address::Address;

fn setup() {
    init_state();
    init_schnorr_master_key();
}

fn derive_address(account: &Account) -> Address {
    let master_key = read_state(|s| s.minter_public_key().cloned().unwrap());
    Address::from(derive_public_key(&master_key, derivation_path(account)).serialize_raw())
}

fn minter_signing_once() -> TestCanisterRuntime {
    TestCanisterRuntime::new().add_signer(sign_as_minter())
}

/// Extracts the transfer amount (in lamports) from a compiled system program
/// transfer instruction. The data layout is:
///   [u32 LE instruction index = 2, u64 LE amount]
fn transfer_amount_from_instruction(instruction: &solana_transaction::CompiledInstruction) -> u64 {
    assert_eq!(instruction.data.len(), 12);
    u64::from_le_bytes(instruction.data[4..12].try_into().unwrap())
}

mod sweep_tests {
    use super::*;
    use crate::test_fixtures::{planned_sweep, queued_deposit_of};

    #[tokio::test]
    async fn should_sign_a_sweep_with_a_single_deposit() {
        setup();
        let account = Account {
            owner: Principal::from_slice(&[1, 2, 3]),
            subaccount: None,
        };
        let amount: Lamport = 500_000_000;
        let blockhash = Hash::new_from_array([0xBB; 32]);
        let sweep = planned_sweep([(0, queued_deposit_of(account, amount))]);
        let runtime = TestCanisterRuntime::new().add_signer(sign_for(&account));

        let (tx, signers) = sign_sweep_transaction(&runtime, &sweep, blockhash)
            .await
            .expect("signing should succeed");

        assert_eq!(signers, vec![Signer::Account(account)]);
        assert_eq!(tx.message.account_keys[0], derive_address(&account));
        assert!(tx.message.account_keys.contains(&MINTER_ADDRESS));
        assert_eq!(tx.message.instructions.len(), 1);
        assert_eq!(
            transfer_amount_from_instruction(&tx.message.instructions[0]),
            amount - FEE_PER_SIGNATURE
        );
        assert_eq!(tx.signatures, vec![account_signature(&account)]);
        assert_eq!(tx.message.recent_blockhash, blockhash);
    }

    #[tokio::test]
    async fn should_sign_with_the_deposits_in_the_order_of_the_message() {
        setup();
        let smaller = Account {
            owner: Principal::from_slice(&[1]),
            subaccount: None,
        };
        let larger = Account {
            owner: Principal::from_slice(&[2]),
            subaccount: None,
        };
        let sweep = planned_sweep([
            (0, queued_deposit_of(smaller, 100_000_000)),
            (1, queued_deposit_of(larger, 200_000_000)),
        ]);
        let runtime = TestCanisterRuntime::new()
            .add_signer(sign_for(&smaller))
            .add_signer(sign_for(&larger));

        let (tx, signers) =
            sign_sweep_transaction(&runtime, &sweep, Hash::new_from_array([0xDD; 32]))
                .await
                .expect("signing should succeed");

        assert_eq!(signers.len(), 2);
        assert_eq!(signers[0], Signer::Account(larger));
        let signer_addresses: Vec<Address> = signers
            .iter()
            .map(|signer| match signer {
                Signer::Account(account) => derive_address(account),
                Signer::Minter => panic!("BUG: the minter does not sign sweeps"),
            })
            .collect();
        assert_eq!(tx.message.account_keys[..2], signer_addresses);
        let signatures: Vec<_> = signers
            .iter()
            .map(|signer| match signer {
                Signer::Account(account) => account_signature(account),
                Signer::Minter => panic!("BUG: the minter does not sign sweeps"),
            })
            .collect();
        assert_eq!(tx.signatures, signatures);
    }

    #[tokio::test]
    async fn should_fail_when_signing_is_rejected() {
        setup();
        let account = Account {
            owner: Principal::from_slice(&[1]),
            subaccount: None,
        };
        let sweep = planned_sweep([(0, queued_deposit_of(account, 500_000_000))]);
        let runtime = TestCanisterRuntime::new().add_signer(sign_for(&account).expect([Err(
            SignCallError::CallFailed(
                CallRejected::with_rejection(4, "signing service unavailable".to_string()).into(),
            ),
        )]));

        let result =
            sign_sweep_transaction(&runtime, &sweep, Hash::new_from_array([0xBB; 32])).await;

        assert_matches!(result, Err(CreateTransferError::SigningFailed(_)));
    }
}

mod batch_withdrawal_tests {
    use super::*;
    use solana_message::{MessageHeader, compiled_instruction::CompiledInstruction};

    const NONCE_ACCOUNT: Address = Address::new_from_array([0x11; 32]);
    const TARGET_1: Address = Address::new_from_array([0xAA; 32]);
    const TARGET_2: Address = Address::new_from_array([0xBB; 32]);

    fn nonce_value() -> Hash {
        Hash::new_from_array([0xCC; 32])
    }

    /// Historical withdrawal events replay only if the builder always compiles to the same
    /// message, so a failure here means a dependency bump broke replay — not that the
    /// expected message needs updating.
    #[test]
    fn should_build_the_recorded_withdrawal_message() {
        let message = build_batch_withdrawal_message(
            &MINTER_ADDRESS,
            &NONCE_ACCOUNT,
            nonce_value(),
            &[(TARGET_1, 100), (TARGET_2, 200)],
        )
        .expect("the message fits within the transaction size limit");

        assert_eq!(
            message,
            Message {
                header: MessageHeader {
                    num_required_signatures: 1,
                    num_readonly_signed_accounts: 0,
                    num_readonly_unsigned_accounts: 2,
                },
                account_keys: vec![
                    MINTER_ADDRESS,
                    NONCE_ACCOUNT,
                    TARGET_1,
                    TARGET_2,
                    solana_system_interface::program::ID,
                    solana_sdk_ids::sysvar::recent_blockhashes::ID,
                ],
                recent_blockhash: nonce_value(),
                instructions: vec![
                    advance_nonce_instruction(),
                    transfer_instruction(2, 100),
                    transfer_instruction(3, 200),
                ],
            }
        );
    }

    #[test]
    fn should_fit_a_full_batch_within_the_maximum_transaction_size() {
        let message = build_batch_withdrawal_message(
            &MINTER_ADDRESS,
            &NONCE_ACCOUNT,
            nonce_value(),
            &targets(MAX_WITHDRAWALS_PER_NONCE_TX),
        )
        .expect("a full batch fits within the transaction size limit");

        let transaction_size = 1 + message.serialize().len() + BYTES_PER_SIGNATURE;
        assert!(
            transaction_size <= MAX_TX_SIZE,
            "transaction size {transaction_size} exceeds {MAX_TX_SIZE}"
        );
    }

    #[test]
    fn should_reject_a_message_exceeding_the_maximum_transaction_size() {
        const NUM_TARGETS: usize = 25;

        let result = build_batch_withdrawal_message(
            &MINTER_ADDRESS,
            &NONCE_ACCOUNT,
            nonce_value(),
            &targets(NUM_TARGETS),
        );

        assert_matches!(
            result,
            Err(CreateTransferError::TransactionTooLarge {
                max: MAX_TX_SIZE,
                ..
            })
        );
    }

    #[test]
    fn should_charge_the_fee_reserved_per_batch() {
        let message = build_batch_withdrawal_message(
            &MINTER_ADDRESS,
            &NONCE_ACCOUNT,
            nonce_value(),
            &targets(MAX_WITHDRAWALS_PER_NONCE_TX),
        )
        .expect("a full batch fits within the transaction size limit");

        assert_eq!(
            VersionedMessage::Legacy(message).transaction_fee(),
            BATCH_WITHDRAWAL_TX_FEE
        );
    }

    #[tokio::test]
    async fn should_sign_the_message_verbatim_with_the_minter_key_only() {
        setup();
        let message = build_batch_withdrawal_message(
            &MINTER_ADDRESS,
            &NONCE_ACCOUNT,
            nonce_value(),
            &[(TARGET_1, 100)],
        )
        .expect("the message fits within the transaction size limit");

        let transaction = sign_batch_withdrawal_message(&minter_signing_once(), message.clone())
            .await
            .expect("signing should succeed");

        assert_eq!(transaction.signatures, vec![minter_signature()]);
        assert_eq!(transaction.message, message);
    }

    #[tokio::test]
    async fn should_fail_when_signing_fails() {
        setup();
        let message = build_batch_withdrawal_message(
            &MINTER_ADDRESS,
            &NONCE_ACCOUNT,
            nonce_value(),
            &[(TARGET_1, 100)],
        )
        .expect("the message fits within the transaction size limit");

        let runtime = TestCanisterRuntime::new().add_signer(sign_as_minter().expect([Err(
            SignCallError::CallFailed(
                CallRejected::with_rejection(4, "signing service unavailable".to_string()).into(),
            ),
        )]));

        let result = sign_batch_withdrawal_message(&runtime, message).await;

        assert!(result.is_err());
    }

    fn targets(count: usize) -> Vec<(Address, Lamport)> {
        (0..count)
            .map(|i| {
                let mut address = [0u8; 32];
                address[0] = i as u8;
                address[1] = (i >> 8) as u8;
                (Address::new_from_array(address), 1_000_000)
            })
            .collect()
    }

    fn advance_nonce_instruction() -> CompiledInstruction {
        const ADVANCE_NONCE_ACCOUNT_DISCRIMINANT: u32 = 4;
        const SYSTEM_PROGRAM_INDEX: u8 = 4;
        const NONCE_ACCOUNT_INDEX: u8 = 1;
        const RECENT_BLOCKHASHES_SYSVAR_INDEX: u8 = 5;
        const MINTER_INDEX: u8 = 0;
        CompiledInstruction {
            program_id_index: SYSTEM_PROGRAM_INDEX,
            accounts: vec![
                NONCE_ACCOUNT_INDEX,
                RECENT_BLOCKHASHES_SYSVAR_INDEX,
                MINTER_INDEX,
            ],
            data: ADVANCE_NONCE_ACCOUNT_DISCRIMINANT.to_le_bytes().to_vec(),
        }
    }

    fn transfer_instruction(to_index: u8, amount: Lamport) -> CompiledInstruction {
        const TRANSFER_DISCRIMINANT: u32 = 2;
        const SYSTEM_PROGRAM_INDEX: u8 = 4;
        const MINTER_INDEX: u8 = 0;
        let mut data = TRANSFER_DISCRIMINANT.to_le_bytes().to_vec();
        data.extend_from_slice(&amount.to_le_bytes());
        CompiledInstruction {
            program_id_index: SYSTEM_PROGRAM_INDEX,
            accounts: vec![MINTER_INDEX, to_index],
            data,
        }
    }
}
