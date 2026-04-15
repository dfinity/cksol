use super::{event::*, *};
use crate::{
    constants::FEE_PER_SIGNATURE as SOLANA_LAMPORTS_PER_SIGNATURE,
    state::{SOLANA_RENT_EXEMPTION_THRESHOLD, audit::process_event, read_state},
    test_fixtures::{
        AUTOMATED_DEPOSIT_FEE, DEPOSIT_CONSOLIDATION_FEE, MANUAL_DEPOSIT_FEE,
        MINIMUM_DEPOSIT_AMOUNT, MINIMUM_WITHDRAWAL_AMOUNT, UPDATE_BALANCE_REQUIRED_CYCLES,
        WITHDRAWAL_FEE, account,
        arb::arb_event,
        deposit_id,
        events::{
            accept_deposit, accept_withdrawal, accept_withdrawal_at, expire_transaction,
            fail_transaction, mint_deposit, resubmit_transaction, submit_withdrawal,
            succeed_transaction,
        },
        init_balance, init_state, ledger_canister_id,
        runtime::TestCanisterRuntime,
        signature, sol_rpc_canister_id, valid_init_args,
    },
    utils::insertion_ordered_map::InsertionOrderedMap,
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
                manual_deposit_fee: MANUAL_DEPOSIT_FEE,
                automated_deposit_fee: AUTOMATED_DEPOSIT_FEE,
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
                pending_withdrawal_requests: BTreeMap::new(),
                sent_withdrawal_requests: BTreeMap::new(),
                successful_withdrawal_requests: BTreeMap::new(),
                failed_withdrawal_requests: BTreeMap::new(),
                deposits_to_consolidate: BTreeMap::new(),
                submitted_transactions: InsertionOrderedMap::new(),
                transactions_to_resubmit: InsertionOrderedMap::new(),
                succeeded_transactions: BTreeSet::new(),
                failed_transactions: InsertionOrderedMap::new(),
                consolidation_transactions: InsertionOrderedMap::new(),
                active_tasks: BTreeSet::new(),
                balance: 0,
            }
        );
    }

    #[test]
    fn should_fail_with_invalid_init_args() {
        fn assert_init_fails(args: InitArgs, expected: &str) {
            let err = State::try_from(args).unwrap_err();
            assert!(
                format!("{err:?}").contains(expected),
                "Expected error containing {expected:?}, got: {err:?}"
            );
        }

        // Anonymous sol_rpc_canister_id
        assert_init_fails(
            InitArgs {
                sol_rpc_canister_id: Principal::anonymous(),
                ..valid_init_args()
            },
            "InvalidCanisterId",
        );

        // Anonymous ledger_canister_id
        assert_init_fails(
            InitArgs {
                ledger_canister_id: Principal::anonymous(),
                ..valid_init_args()
            },
            "InvalidCanisterId",
        );

        // Identical canister IDs
        assert_init_fails(
            InitArgs {
                sol_rpc_canister_id: sol_rpc_canister_id(),
                ledger_canister_id: sol_rpc_canister_id(),
                ..valid_init_args()
            },
            "InvalidCanisterId",
        );

        // automated_deposit_fee below manual_deposit_fee
        assert_init_fails(
            InitArgs {
                manual_deposit_fee: AUTOMATED_DEPOSIT_FEE + 1,
                ..valid_init_args()
            },
            "InvalidAutomatedDepositFee",
        );

        // automated_deposit_fee exceeds minimum_deposit_amount
        assert_init_fails(
            InitArgs {
                automated_deposit_fee: MINIMUM_DEPOSIT_AMOUNT + 1,
                manual_deposit_fee: MINIMUM_DEPOSIT_AMOUNT, // manual_deposit_fee == minimum, no overlap
                ..valid_init_args()
            },
            "InvalidMinimumDepositAmount",
        );

        // minimum_deposit_amount below automated_deposit_fee
        assert_init_fails(
            InitArgs {
                minimum_deposit_amount: AUTOMATED_DEPOSIT_FEE - 1,
                manual_deposit_fee: AUTOMATED_DEPOSIT_FEE - 1, // manual_deposit_fee == minimum, no overlap
                ..valid_init_args()
            },
            "InvalidMinimumDepositAmount",
        );

        // minimum_withdrawal_amount below withdrawal_fee + rent exemption threshold
        assert_init_fails(
            InitArgs {
                minimum_withdrawal_amount: WITHDRAWAL_FEE + SOLANA_RENT_EXEMPTION_THRESHOLD - 1,
                ..valid_init_args()
            },
            "InvalidMinimumWithdrawalAmount",
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
    fn should_update_manual_deposit_fee() {
        let mut state = initial_state();
        let new_deposit_fee = MANUAL_DEPOSIT_FEE / 2;

        state
            .upgrade(UpgradeArgs {
                manual_deposit_fee: Some(new_deposit_fee),
                ..Default::default()
            })
            .unwrap();

        assert_eq!(state.manual_deposit_fee(), new_deposit_fee);
    }

    #[test]
    fn should_update_automated_deposit_fee() {
        let mut state = initial_state();
        let new_automated_fee = AUTOMATED_DEPOSIT_FEE / 2;

        state
            .upgrade(UpgradeArgs {
                automated_deposit_fee: Some(new_automated_fee),
                ..Default::default()
            })
            .unwrap();

        assert_eq!(state.automated_deposit_fee(), new_automated_fee);
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
    fn should_fail_with_invalid_upgrade_args() {
        fn assert_upgrade_fails(args: UpgradeArgs, expected: &str) {
            let err = initial_state().upgrade(args).unwrap_err();
            assert!(
                format!("{err:?}").contains(expected),
                "Expected error containing {expected:?}, got: {err:?}"
            );
        }

        // Anonymous sol_rpc_canister_id
        assert_upgrade_fails(
            UpgradeArgs {
                sol_rpc_canister_id: Some(Principal::anonymous()),
                ..Default::default()
            },
            "InvalidCanisterId",
        );
        // automated_deposit_fee below manual_deposit_fee
        assert_upgrade_fails(
            UpgradeArgs {
                manual_deposit_fee: Some(AUTOMATED_DEPOSIT_FEE + 1),
                ..Default::default()
            },
            "InvalidAutomatedDepositFee",
        );
        // automated_deposit_fee exceeds minimum_deposit_amount
        assert_upgrade_fails(
            UpgradeArgs {
                automated_deposit_fee: Some(MINIMUM_DEPOSIT_AMOUNT + 1),
                manual_deposit_fee: Some(MINIMUM_DEPOSIT_AMOUNT), // manual_deposit_fee == minimum, no overlap
                ..Default::default()
            },
            "InvalidMinimumDepositAmount",
        );
        // minimum_deposit_amount below automated_deposit_fee
        assert_upgrade_fails(
            UpgradeArgs {
                minimum_deposit_amount: Some(AUTOMATED_DEPOSIT_FEE - 1),
                manual_deposit_fee: Some(AUTOMATED_DEPOSIT_FEE - 1), // manual_deposit_fee == minimum, no overlap
                ..Default::default()
            },
            "InvalidMinimumDepositAmount",
        );
        // withdrawal_fee makes minimum_withdrawal_amount too low
        assert_upgrade_fails(
            UpgradeArgs {
                withdrawal_fee: Some(MINIMUM_WITHDRAWAL_AMOUNT),
                ..Default::default()
            },
            "InvalidMinimumWithdrawalAmount",
        );
        // minimum_withdrawal_amount below withdrawal_fee + rent exemption threshold
        assert_upgrade_fails(
            UpgradeArgs {
                minimum_withdrawal_amount: Some(
                    WITHDRAWAL_FEE + SOLANA_RENT_EXEMPTION_THRESHOLD - 1,
                ),
                ..Default::default()
            },
            "InvalidMinimumWithdrawalAmount",
        );
    }

    #[test]
    fn should_succeed_when_minimum_deposit_amount_equals_max_deposit_fee() {
        let mut state = initial_state();

        state
            .upgrade(UpgradeArgs {
                minimum_deposit_amount: Some(AUTOMATED_DEPOSIT_FEE),
                ..Default::default()
            })
            .unwrap();

        assert_eq!(state.minimum_deposit_amount(), AUTOMATED_DEPOSIT_FEE);
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
        let new_minimum_deposit_amount = MANUAL_DEPOSIT_FEE - 1;

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

        // Expire then resubmit the transaction with a new signature
        expire_transaction(signature(0xAA));
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
