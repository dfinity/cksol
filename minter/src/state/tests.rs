use super::{event::*, *};
use crate::utils::insertion_ordered_map::InsertionOrderedMap;
use crate::{
    constants::FEE_PER_SIGNATURE as SOLANA_LAMPORTS_PER_SIGNATURE,
    state::{SOLANA_RENT_EXEMPTION_THRESHOLD, audit::process_event, read_state},
    test_fixtures::{
        DEPOSIT_CONSOLIDATION_FEE, DEPOSIT_FEE, MINIMUM_DEPOSIT_AMOUNT, MINIMUM_WITHDRAWAL_AMOUNT,
        UPDATE_BALANCE_REQUIRED_CYCLES, WITHDRAWAL_FEE, account,
        arb::arb_event,
        deposit_id,
        events::{
            accept_deposit, accept_withdrawal, accept_withdrawal_at, fail_transaction,
            mint_deposit, resubmit_transaction, submit_withdrawal, succeed_transaction,
        },
        init_balance, init_state, ledger_canister_id,
        runtime::TestCanisterRuntime,
        signature, sol_rpc_canister_id, valid_init_args,
    },
};
use assert_matches::assert_matches;
use cksol_types_internal::SolanaNetwork;
use cksol_types_internal::{Ed25519KeyName, InitArgs, UpgradeArgs};
use ic_stable_structures::Storable;
use proptest::prelude::*;
use std::borrow::Cow;

proptest! {
    #[test]
    fn event_minicbor_roundtrip(event in arb_event()) {
        let bytes = event.to_bytes();
        let decoded = Event::from_bytes(Cow::Borrowed(&bytes));
        assert_eq!(event, decoded);
    }
}

mod state_from_init_args {
    use super::*;

    #[test]
    fn should_succeed_with_valid_args() {
        let state = State::try_from(valid_init_args()).unwrap();

        assert_eq!(
            state,
            State {
                minter_public_key: None,
                master_key_name: Ed25519KeyName::MainnetProdKey1,
                ledger_canister_id: ledger_canister_id(),
                sol_rpc_canister_id: sol_rpc_canister_id(),
                solana_network: SolanaNetwork::Mainnet,
                deposit_fee: DEPOSIT_FEE,
                deposit_consolidation_fee: DEPOSIT_CONSOLIDATION_FEE,
                withdrawal_fee: WITHDRAWAL_FEE,
                minimum_withdrawal_amount: MINIMUM_WITHDRAWAL_AMOUNT,
                minimum_deposit_amount: MINIMUM_DEPOSIT_AMOUNT,
                update_balance_required_cycles: UPDATE_BALANCE_REQUIRED_CYCLES,
                pending_update_balance_requests: BTreeSet::new(),
                pending_withdrawal_request_guards: BTreeSet::new(),
                accepted_deposits: InsertionOrderedMap::new(),
                quarantined_deposits: InsertionOrderedMap::new(),
                minted_deposits: InsertionOrderedMap::new(),
                pending_withdrawal_requests: InsertionOrderedMap::new(),
                sent_withdrawal_requests: InsertionOrderedMap::new(),
                successful_withdrawal_requests: InsertionOrderedMap::new(),
                failed_withdrawal_requests: InsertionOrderedMap::new(),
                deposits_to_consolidate: InsertionOrderedMap::new(),
                submitted_transactions: InsertionOrderedMap::new(),
                succeeded_transactions: BTreeSet::new(),
                failed_transactions: InsertionOrderedMap::new(),
                consolidation_transactions: InsertionOrderedMap::new(),
                active_tasks: BTreeSet::new(),
                balance: 0,
            }
        );
    }

    #[test]
    fn should_fail_when_any_canister_id_is_anonymous() {
        assert_matches!(
            State::try_from(InitArgs {
                sol_rpc_canister_id: Principal::anonymous(),
                ..valid_init_args()
            }),
            Err(InvalidStateError::InvalidCanisterId(_))
        );

        assert_matches!(
            State::try_from(InitArgs {
                ledger_canister_id: Principal::anonymous(),
                ..valid_init_args()
            }),
            Err(InvalidStateError::InvalidCanisterId(_))
        );

        assert_matches!(
            State::try_from(InitArgs {
                sol_rpc_canister_id: Principal::anonymous(),
                ledger_canister_id: Principal::anonymous(),
                ..valid_init_args()
            }),
            Err(InvalidStateError::InvalidCanisterId(_))
        );
    }

    #[test]
    fn should_fail_when_canister_ids_are_identical() {
        let same_id = sol_rpc_canister_id();
        let args = InitArgs {
            sol_rpc_canister_id: same_id,
            ledger_canister_id: same_id,
            ..valid_init_args()
        };

        assert_matches!(
            State::try_from(args),
            Err(InvalidStateError::InvalidCanisterId(_))
        );
    }

    #[test]
    fn should_fail_when_minimum_withdrawal_amount_too_low() {
        let minimum_required = WITHDRAWAL_FEE + SOLANA_RENT_EXEMPTION_THRESHOLD;
        let insufficient_minimum_withdrawal_amount = minimum_required - 1;
        let args = InitArgs {
            minimum_withdrawal_amount: insufficient_minimum_withdrawal_amount,
            ..valid_init_args()
        };

        assert_eq!(
            State::try_from(args),
            Err(InvalidStateError::InvalidMinimumWithdrawalAmount {
                minimum_withdrawal_amount: insufficient_minimum_withdrawal_amount,
                withdrawal_fee: WITHDRAWAL_FEE,
                rent_exemption_threshold: SOLANA_RENT_EXEMPTION_THRESHOLD,
            })
        );
    }

    #[test]
    fn should_fail_when_minimum_deposit_amount_too_low() {
        let insufficient_minimum_deposit_amount = DEPOSIT_FEE / 2;
        let args = InitArgs {
            minimum_deposit_amount: insufficient_minimum_deposit_amount,
            ..valid_init_args()
        };
        assert_matches!(
            State::try_from(args),
            Err(InvalidStateError::InvalidMinimumDepositAmount {
                minimum_deposit_amount,
                deposit_fee
            }) if minimum_deposit_amount == insufficient_minimum_deposit_amount && deposit_fee == DEPOSIT_FEE
        );
    }

    #[test]
    fn should_succeed_when_minimum_withdrawal_amount_equals_required_minimum() {
        let minimum_required = WITHDRAWAL_FEE + SOLANA_RENT_EXEMPTION_THRESHOLD;
        let args = InitArgs {
            minimum_withdrawal_amount: minimum_required,
            ..valid_init_args()
        };

        let state = State::try_from(args).unwrap();
        assert_eq!(state.minimum_withdrawal_amount(), minimum_required);
    }
}

mod state_upgrade {
    use super::*;

    fn initial_state() -> State {
        State::try_from(valid_init_args()).unwrap()
    }

    #[test]
    fn should_update_sol_rpc_canister_id() {
        let mut state = initial_state();
        let new_canister_id = Principal::from_slice(&[3_u8; 20]);

        state
            .upgrade(UpgradeArgs {
                sol_rpc_canister_id: Some(new_canister_id),
                ..Default::default()
            })
            .unwrap();

        assert_eq!(state.sol_rpc_canister_id(), new_canister_id);
    }

    #[test]
    fn should_update_deposit_fee() {
        let mut state = initial_state();
        let new_deposit_fee = DEPOSIT_FEE / 2;

        state
            .upgrade(UpgradeArgs {
                deposit_fee: Some(new_deposit_fee),
                ..Default::default()
            })
            .unwrap();

        assert_eq!(state.deposit_fee(), new_deposit_fee);
    }

    #[test]
    fn should_update_minimum_deposit_amount() {
        let mut state = initial_state();
        let new_minimum_deposit_amount = MINIMUM_DEPOSIT_AMOUNT * 2;

        state
            .upgrade(UpgradeArgs {
                minimum_deposit_amount: Some(new_minimum_deposit_amount),
                ..Default::default()
            })
            .unwrap();

        assert_eq!(state.minimum_deposit_amount(), new_minimum_deposit_amount);
    }

    #[test]
    fn should_update_minimum_withdrawal_amount() {
        let mut state = initial_state();
        let new_minimum_withdrawal_amount = MINIMUM_WITHDRAWAL_AMOUNT * 2;

        state
            .upgrade(UpgradeArgs {
                minimum_withdrawal_amount: Some(new_minimum_withdrawal_amount),
                ..Default::default()
            })
            .unwrap();

        assert_eq!(
            state.minimum_withdrawal_amount(),
            new_minimum_withdrawal_amount
        );
    }

    #[test]
    fn should_fail_when_new_sol_rpc_canister_id_is_anonymous() {
        let mut state = initial_state();

        assert_matches!(
            state.upgrade(UpgradeArgs {
                sol_rpc_canister_id: Some(Principal::anonymous()),
                ..Default::default()
            }),
            Err(InvalidStateError::InvalidCanisterId(_))
        );
    }

    #[test]
    fn should_fail_when_new_deposit_fee_exceeds_minimum_deposit_amount() {
        let mut state = initial_state();
        let new_deposit_fee = MINIMUM_DEPOSIT_AMOUNT + 1;

        assert_matches!(
            state.upgrade(UpgradeArgs {
                deposit_fee: Some(new_deposit_fee),
                ..Default::default()
            }),
            Err(InvalidStateError::InvalidMinimumDepositAmount {
                minimum_deposit_amount,
                deposit_fee
            }) if minimum_deposit_amount == MINIMUM_DEPOSIT_AMOUNT && deposit_fee == new_deposit_fee
        );
    }

    #[test]
    fn should_fail_when_new_minimum_deposit_amount_below_deposit_fee() {
        let mut state = initial_state();
        let new_minimum_deposit_amount = DEPOSIT_FEE - 1;

        assert_matches!(
            state.upgrade(UpgradeArgs {
                minimum_deposit_amount: Some(new_minimum_deposit_amount),
                ..Default::default()
            }),
            Err(InvalidStateError::InvalidMinimumDepositAmount {
                minimum_deposit_amount,
                deposit_fee
            }) if minimum_deposit_amount == new_minimum_deposit_amount && deposit_fee == DEPOSIT_FEE
        );
    }

    #[test]
    fn should_succeed_when_minimum_deposit_amount_equals_deposit_fee() {
        let mut state = initial_state();

        state
            .upgrade(UpgradeArgs {
                minimum_deposit_amount: Some(DEPOSIT_FEE),
                ..Default::default()
            })
            .unwrap();

        assert_eq!(state.minimum_deposit_amount(), DEPOSIT_FEE);
    }

    #[test]
    fn should_update_withdrawal_fee() {
        let mut state = initial_state();
        let new_withdrawal_fee = WITHDRAWAL_FEE / 2;

        state
            .upgrade(UpgradeArgs {
                withdrawal_fee: Some(new_withdrawal_fee),
                ..Default::default()
            })
            .unwrap();

        assert_eq!(state.withdrawal_fee(), new_withdrawal_fee);
    }

    #[test]
    fn should_fail_when_new_withdrawal_fee_makes_minimum_withdrawal_amount_invalid() {
        let mut state = initial_state();
        // Set withdrawal_fee such that withdrawal_fee + rent_exemption > minimum_withdrawal_amount
        let new_withdrawal_fee = MINIMUM_WITHDRAWAL_AMOUNT;

        assert_eq!(
            state.upgrade(UpgradeArgs {
                withdrawal_fee: Some(new_withdrawal_fee),
                ..Default::default()
            }),
            Err(InvalidStateError::InvalidMinimumWithdrawalAmount {
                minimum_withdrawal_amount: MINIMUM_WITHDRAWAL_AMOUNT,
                withdrawal_fee: new_withdrawal_fee,
                rent_exemption_threshold: SOLANA_RENT_EXEMPTION_THRESHOLD,
            })
        );
    }

    #[test]
    fn should_fail_when_new_minimum_withdrawal_amount_below_required_minimum() {
        let mut state = initial_state();
        let minimum_required = WITHDRAWAL_FEE + SOLANA_RENT_EXEMPTION_THRESHOLD;
        let new_minimum_withdrawal_amount = minimum_required - 1;

        assert_eq!(
            state.upgrade(UpgradeArgs {
                minimum_withdrawal_amount: Some(new_minimum_withdrawal_amount),
                ..Default::default()
            }),
            Err(InvalidStateError::InvalidMinimumWithdrawalAmount {
                minimum_withdrawal_amount: new_minimum_withdrawal_amount,
                withdrawal_fee: WITHDRAWAL_FEE,
                rent_exemption_threshold: SOLANA_RENT_EXEMPTION_THRESHOLD,
            })
        );
    }

    #[test]
    fn should_succeed_when_minimum_withdrawal_amount_equals_required_minimum() {
        let mut state = initial_state();
        let minimum_required = WITHDRAWAL_FEE + SOLANA_RENT_EXEMPTION_THRESHOLD;

        state
            .upgrade(UpgradeArgs {
                minimum_withdrawal_amount: Some(minimum_required),
                ..Default::default()
            })
            .unwrap();

        assert_eq!(state.minimum_withdrawal_amount(), minimum_required);
    }

    #[test]
    fn should_update_update_balance_required_cycles() {
        let mut state = initial_state();
        let new_update_balance_required_cycles = (UPDATE_BALANCE_REQUIRED_CYCLES * 2) as u64;

        state
            .upgrade(UpgradeArgs {
                update_balance_required_cycles: Some(new_update_balance_required_cycles),
                ..Default::default()
            })
            .unwrap();

        assert_eq!(
            state.update_balance_required_cycles(),
            new_update_balance_required_cycles as u128
        );
    }

    // This test ensures the canister state is rolled back after a failed upgrade
    #[test]
    #[should_panic = "InvalidMinimumDepositAmount"]
    fn should_panic_when_upgrade_fails() {
        let mut state = initial_state();
        let new_minimum_deposit_amount = DEPOSIT_FEE - 1;

        process_event(
            &mut state,
            EventType::Upgrade(UpgradeArgs {
                minimum_deposit_amount: Some(new_minimum_deposit_amount),
                ..Default::default()
            }),
            &TestCanisterRuntime::new().with_time(0).with_time(0),
        );
    }
}

#[test]
fn should_track_balance_through_deposits_withdrawals_and_failures() {
    const DEPOSIT_1: u64 = 500_000_000;
    const DEPOSIT_2: u64 = 300_000_000;
    const DEPOSIT_3: u64 = 200_000_000;
    const WITHDRAWAL_1: u64 = 50_000_000;
    const WITHDRAWAL_2: u64 = 80_000_000;
    const TRANSFER_1: u64 = WITHDRAWAL_1 - WITHDRAWAL_FEE;
    const TRANSFER_2: u64 = WITHDRAWAL_2 - WITHDRAWAL_FEE;

    /// Creates a Solana message with the given number of required signatures.
    fn message_with_signers(num_signers: u8) -> solana_message::Message {
        solana_message::Message {
            header: solana_message::MessageHeader {
                num_required_signatures: num_signers,
                num_readonly_signed_accounts: 0,
                num_readonly_unsigned_accounts: 0,
            },
            account_keys: vec![],
            recent_blockhash: Default::default(),
            instructions: vec![],
        }
    }

    fn submit_transaction(sig: Signature, num_signers: u8, purpose: TransactionPurpose) {
        let signers: Vec<_> = (0..num_signers).map(|i| account(100 + i)).collect();
        mutate_state(|state| {
            process_event(
                state,
                EventType::SubmittedTransaction {
                    signature: sig,
                    message: message_with_signers(num_signers).into(),
                    signers,
                    slot: 0,
                    purpose,
                },
                &TestCanisterRuntime::new().with_time(0).with_time(0),
            )
        });
    }

    init_state();
    assert_eq!(read_state(|s| s.balance()), 0);

    // Accepting and minting deposits does not change the balance
    accept_deposit(deposit_id(1), DEPOSIT_1);
    accept_deposit(deposit_id(2), DEPOSIT_2);
    mint_deposit(deposit_id(1), 0);
    mint_deposit(deposit_id(2), 1);
    assert_eq!(read_state(|s| s.balance()), 0);

    // Submitting a consolidation (2 signers) does not change the balance
    submit_transaction(
        signature(0xAA),
        2,
        TransactionPurpose::ConsolidateDeposits {
            mint_indices: vec![0.into(), 1.into()],
        },
    );
    assert_eq!(read_state(|s| s.balance()), 0);

    // Finalized consolidation: balance += total_deposits - tx_fee(2 signers)
    succeed_transaction(signature(0xAA));
    let expected = DEPOSIT_1 + DEPOSIT_2 - 2 * SOLANA_LAMPORTS_PER_SIGNATURE;
    assert_eq!(read_state(|s| s.balance()), expected);

    // Accepting withdrawals does not change the balance
    accept_withdrawal(account(3), 0, WITHDRAWAL_1);
    accept_withdrawal(account(4), 1, WITHDRAWAL_2);
    assert_eq!(read_state(|s| s.balance()), expected);

    // Submitting a withdrawal (1 signer): balance -= total_transfers + tx_fee
    submit_transaction(
        signature(0xBB),
        1,
        TransactionPurpose::WithdrawSol {
            burn_indices: vec![0.into(), 1.into()],
        },
    );
    let expected = expected - TRANSFER_1 - TRANSFER_2 - SOLANA_LAMPORTS_PER_SIGNATURE;
    assert_eq!(read_state(|s| s.balance()), expected);

    // Finalizing a withdrawal does not change the balance
    succeed_transaction(signature(0xBB));
    assert_eq!(read_state(|s| s.balance()), expected);

    // Failed consolidation does not credit the balance
    accept_deposit(deposit_id(3), DEPOSIT_3);
    mint_deposit(deposit_id(3), 2);
    submit_transaction(
        signature(0xCC),
        1,
        TransactionPurpose::ConsolidateDeposits {
            mint_indices: vec![2.into()],
        },
    );
    fail_transaction(signature(0xCC));
    assert_eq!(read_state(|s| s.balance()), expected);
}

mod oldest_incomplete_withdrawal_created_at {
    use super::*;

    const AMOUNT: u64 = 50_000_000;

    #[test]
    fn should_be_none_when_no_incomplete_withdrawals() {
        init_state();
        assert_eq!(
            read_state(|s| s.oldest_incomplete_withdrawal_created_at()),
            None
        );
    }

    #[test]
    fn should_return_timestamp_of_single_pending_withdrawal() {
        init_state();
        init_balance();
        accept_withdrawal_at(account(1), 0, AMOUNT, 1_000_000_000);
        assert_eq!(
            read_state(|s| s.oldest_incomplete_withdrawal_created_at()),
            Some(1_000_000_000)
        );
    }

    #[test]
    fn should_return_oldest_timestamp_with_multiple_pending_withdrawals() {
        init_state();
        init_balance();
        accept_withdrawal_at(account(1), 0, AMOUNT, 1_000_000_000);
        accept_withdrawal_at(account(2), 1, AMOUNT, 2_000_000_000);
        accept_withdrawal_at(account(3), 2, AMOUNT, 3_000_000_000);
        assert_eq!(
            read_state(|s| s.oldest_incomplete_withdrawal_created_at()),
            Some(1_000_000_000)
        );
    }

    #[test]
    fn should_persist_through_submission() {
        init_state();
        init_balance();
        accept_withdrawal_at(account(1), 0, AMOUNT, 1_000_000_000);
        accept_withdrawal_at(account(2), 1, AMOUNT, 2_000_000_000);

        submit_withdrawal(signature(0xAA), account(100), 0, vec![0, 1]);

        // Both withdrawals are now sent but still incomplete
        assert_eq!(
            read_state(|s| s.oldest_incomplete_withdrawal_created_at()),
            Some(1_000_000_000)
        );
    }

    #[test]
    fn should_update_when_oldest_withdrawal_succeeds() {
        init_state();
        init_balance();
        accept_withdrawal_at(account(1), 0, AMOUNT, 1_000_000_000);
        accept_withdrawal_at(account(2), 1, AMOUNT, 2_000_000_000);

        submit_withdrawal(signature(0xAA), account(100), 0, vec![0]);
        submit_withdrawal(signature(0xBB), account(100), 0, vec![1]);
        succeed_transaction(signature(0xAA));

        assert_eq!(
            read_state(|s| s.oldest_incomplete_withdrawal_created_at()),
            Some(2_000_000_000)
        );
    }

    #[test]
    fn should_update_when_oldest_withdrawal_fails() {
        init_state();
        init_balance();
        accept_withdrawal_at(account(1), 0, AMOUNT, 1_000_000_000);
        accept_withdrawal_at(account(2), 1, AMOUNT, 2_000_000_000);

        submit_withdrawal(signature(0xAA), account(100), 0, vec![0]);
        submit_withdrawal(signature(0xBB), account(100), 0, vec![1]);
        fail_transaction(signature(0xAA));

        assert_eq!(
            read_state(|s| s.oldest_incomplete_withdrawal_created_at()),
            Some(2_000_000_000)
        );
    }

    #[test]
    fn should_be_none_when_all_withdrawals_are_finalized() {
        init_state();
        init_balance();
        accept_withdrawal_at(account(1), 0, AMOUNT, 1_000_000_000);
        accept_withdrawal_at(account(2), 1, AMOUNT, 2_000_000_000);

        submit_withdrawal(signature(0xAA), account(100), 0, vec![0, 1]);
        succeed_transaction(signature(0xAA));

        assert_eq!(
            read_state(|s| s.oldest_incomplete_withdrawal_created_at()),
            None
        );
    }

    #[test]
    fn should_preserve_created_at_through_resubmission() {
        init_state();
        init_balance();
        accept_withdrawal_at(account(1), 0, AMOUNT, 1_000_000_000);
        accept_withdrawal_at(account(2), 1, AMOUNT, 2_000_000_000);

        submit_withdrawal(signature(0xAA), account(100), 0, vec![0, 1]);

        assert_eq!(
            read_state(|s| s.oldest_incomplete_withdrawal_created_at()),
            Some(1_000_000_000)
        );

        // Resubmit the transaction with a new signature
        resubmit_transaction(signature(0xAA), signature(0xBB), 42);

        // created_at timestamps should be unchanged
        assert_eq!(
            read_state(|s| s.oldest_incomplete_withdrawal_created_at()),
            Some(1_000_000_000)
        );

        // Finalize the resubmitted transaction
        succeed_transaction(signature(0xBB));

        assert_eq!(
            read_state(|s| s.oldest_incomplete_withdrawal_created_at()),
            None
        );
    }
}
