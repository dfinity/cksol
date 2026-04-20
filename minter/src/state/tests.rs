use super::{event::*, *};
use crate::{
    constants::{FEE_PER_SIGNATURE, RENT_EXEMPTION_THRESHOLD},
    state::{audit::process_event, read_state},
    test_fixtures::{
        AUTOMATED_DEPOSIT_FEE, DEPOSIT_CONSOLIDATION_FEE, MANUAL_DEPOSIT_FEE,
        MINIMUM_DEPOSIT_AMOUNT, MINIMUM_WITHDRAWAL_AMOUNT, PROCESS_DEPOSIT_REQUIRED_CYCLES,
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
use cksol_types_internal::{Ed25519KeyName, InitArgs, SolanaNetwork, UpgradeArgs};
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

mod state_validation {
    use super::*;

    #[test]
    fn should_fail_with_invalid_args() {
        // manual_deposit_fee exceeds automated_deposit_fee
        assert_fails_both(
            InitArgs {
                manual_deposit_fee: AUTOMATED_DEPOSIT_FEE + 1,
                ..valid_init_args()
            },
            UpgradeArgs {
                manual_deposit_fee: Some(AUTOMATED_DEPOSIT_FEE + 1),
                ..Default::default()
            },
            |e| matches!(e, InvalidStateError::InvalidDepositFees { .. }),
        );
        // automated_deposit_fee below manual_deposit_fee
        assert_fails_both(
            InitArgs {
                automated_deposit_fee: MANUAL_DEPOSIT_FEE - 1,
                ..valid_init_args()
            },
            UpgradeArgs {
                automated_deposit_fee: Some(MANUAL_DEPOSIT_FEE - 1),
                ..Default::default()
            },
            |e| matches!(e, InvalidStateError::InvalidDepositFees { .. }),
        );
        // automated_deposit_fee exceeds minimum_deposit_amount
        assert_fails_both(
            InitArgs {
                automated_deposit_fee: MINIMUM_DEPOSIT_AMOUNT + 1,
                ..valid_init_args()
            },
            UpgradeArgs {
                automated_deposit_fee: Some(MINIMUM_DEPOSIT_AMOUNT + 1),
                ..Default::default()
            },
            |e| matches!(e, InvalidStateError::InvalidDepositFees { .. }),
        );
        // minimum_deposit_amount below automated_deposit_fee
        assert_fails_both(
            InitArgs {
                minimum_deposit_amount: AUTOMATED_DEPOSIT_FEE - 1,
                ..valid_init_args()
            },
            UpgradeArgs {
                minimum_deposit_amount: Some(AUTOMATED_DEPOSIT_FEE - 1),
                ..Default::default()
            },
            |e| matches!(e, InvalidStateError::InvalidDepositFees { .. }),
        );
        // minimum_deposit_amount below sol_transfer_fee + rent exemption threshold
        // (automated_deposit_fee and manual_deposit_fee set to 1 to isolate this condition)
        let minimum_required = FEE_PER_SIGNATURE + RENT_EXEMPTION_THRESHOLD;
        assert_fails_both(
            InitArgs {
                automated_deposit_fee: 1,
                manual_deposit_fee: 1,
                minimum_deposit_amount: minimum_required - 1,
                ..valid_init_args()
            },
            UpgradeArgs {
                automated_deposit_fee: Some(1),
                manual_deposit_fee: Some(1),
                minimum_deposit_amount: Some(minimum_required - 1),
                ..Default::default()
            },
            |e| matches!(e, InvalidStateError::InvalidMinimumDepositAmount { .. }),
        );
        // withdrawal_fee exceeds minimum_withdrawal_amount - rent exemption threshold
        assert_fails_both(
            InitArgs {
                withdrawal_fee: MINIMUM_WITHDRAWAL_AMOUNT - RENT_EXEMPTION_THRESHOLD + 1,
                ..valid_init_args()
            },
            UpgradeArgs {
                withdrawal_fee: Some(MINIMUM_WITHDRAWAL_AMOUNT - RENT_EXEMPTION_THRESHOLD + 1),
                ..Default::default()
            },
            |e| matches!(e, InvalidStateError::InvalidMinimumWithdrawalAmount { .. }),
        );
        // minimum_withdrawal_amount below withdrawal_fee + rent exemption threshold
        assert_fails_both(
            InitArgs {
                minimum_withdrawal_amount: WITHDRAWAL_FEE + RENT_EXEMPTION_THRESHOLD - 1,
                ..valid_init_args()
            },
            UpgradeArgs {
                minimum_withdrawal_amount: Some(WITHDRAWAL_FEE + RENT_EXEMPTION_THRESHOLD - 1),
                ..Default::default()
            },
            |e| matches!(e, InvalidStateError::InvalidMinimumWithdrawalAmount { .. }),
        );
    }

    #[test]
    fn should_succeed_at_boundary_conditions() {
        // manual_deposit_fee can equal automated_deposit_fee
        assert_succeeds_both(
            InitArgs {
                manual_deposit_fee: AUTOMATED_DEPOSIT_FEE,
                ..valid_init_args()
            },
            UpgradeArgs {
                manual_deposit_fee: Some(AUTOMATED_DEPOSIT_FEE),
                ..Default::default()
            },
        );
        // minimum_deposit_amount can equal automated_deposit_fee
        assert_succeeds_both(
            InitArgs {
                minimum_deposit_amount: AUTOMATED_DEPOSIT_FEE,
                ..valid_init_args()
            },
            UpgradeArgs {
                minimum_deposit_amount: Some(AUTOMATED_DEPOSIT_FEE),
                ..Default::default()
            },
        );
        // minimum_deposit_amount can equal sol_transfer_fee + rent exemption threshold
        let minimum_required = FEE_PER_SIGNATURE + RENT_EXEMPTION_THRESHOLD;
        assert_succeeds_both(
            InitArgs {
                automated_deposit_fee: 1,
                manual_deposit_fee: 1,
                minimum_deposit_amount: minimum_required,
                ..valid_init_args()
            },
            UpgradeArgs {
                automated_deposit_fee: Some(1),
                manual_deposit_fee: Some(1),
                minimum_deposit_amount: Some(minimum_required),
                ..Default::default()
            },
        );
        // minimum_withdrawal_amount can equal withdrawal_fee + rent exemption threshold
        let minimum_required = WITHDRAWAL_FEE + RENT_EXEMPTION_THRESHOLD;
        assert_succeeds_both(
            InitArgs {
                minimum_withdrawal_amount: minimum_required,
                ..valid_init_args()
            },
            UpgradeArgs {
                minimum_withdrawal_amount: Some(minimum_required),
                ..Default::default()
            },
        );
    }

    fn assert_fails_both(
        init_args: InitArgs,
        upgrade_args: UpgradeArgs,
        check: impl Fn(&InvalidStateError) -> bool + Copy,
    ) {
        let err = State::try_from(init_args).unwrap_err();
        assert!(check(&err), "init: unexpected error: {err:?}");
        let mut state = State::try_from(valid_init_args()).unwrap();
        let err = state.upgrade(upgrade_args).unwrap_err();
        assert!(check(&err), "upgrade: unexpected error: {err:?}");
    }

    fn assert_succeeds_both(init_args: InitArgs, upgrade_args: UpgradeArgs) {
        State::try_from(init_args).unwrap();
        let mut state = State::try_from(valid_init_args()).unwrap();
        state.upgrade(upgrade_args).unwrap();
    }
}

mod state_from_init_args {
    use super::*;

    #[test]
    fn should_succeed() {
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
                process_deposit_required_cycles: PROCESS_DEPOSIT_REQUIRED_CYCLES,
                monitored_accounts: BTreeSet::new(),
                pending_process_deposit_request_guards: BTreeSet::new(),
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
    fn should_fail_with_invalid_canister_ids() {
        fn assert_init_fails(args: InitArgs, check: impl Fn(&InvalidStateError) -> bool) {
            let err = State::try_from(args).unwrap_err();
            assert!(check(&err), "unexpected error: {err:?}");
        }
        // Anonymous sol_rpc_canister_id
        assert_init_fails(
            InitArgs {
                sol_rpc_canister_id: Principal::anonymous(),
                ..valid_init_args()
            },
            |e| matches!(e, InvalidStateError::InvalidCanisterId(_)),
        );
        // Anonymous ledger_canister_id
        assert_init_fails(
            InitArgs {
                ledger_canister_id: Principal::anonymous(),
                ..valid_init_args()
            },
            |e| matches!(e, InvalidStateError::InvalidCanisterId(_)),
        );
        // Identical canister IDs
        assert_init_fails(
            InitArgs {
                sol_rpc_canister_id: sol_rpc_canister_id(),
                ledger_canister_id: sol_rpc_canister_id(),
                ..valid_init_args()
            },
            |e| matches!(e, InvalidStateError::InvalidCanisterId(_)),
        );
    }
}

mod state_upgrade {
    use super::*;

    fn initial_state() -> State {
        State::try_from(valid_init_args()).unwrap()
    }

    #[test]
    fn should_update_fields() {
        let new_canister_id = Principal::from_slice(&[3_u8; 20]);
        let new_manual_fee = MANUAL_DEPOSIT_FEE / 2;
        let new_automated_fee = AUTOMATED_DEPOSIT_FEE / 2;
        let new_minimum_deposit_amount = MINIMUM_DEPOSIT_AMOUNT * 2;
        let new_minimum_withdrawal_amount = MINIMUM_WITHDRAWAL_AMOUNT * 2;
        let new_withdrawal_fee = WITHDRAWAL_FEE / 2;
        let new_process_deposit_required_cycles = (PROCESS_DEPOSIT_REQUIRED_CYCLES * 2) as u64;

        let mut state = initial_state();
        state
            .upgrade(UpgradeArgs {
                sol_rpc_canister_id: Some(new_canister_id),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(state.sol_rpc_canister_id(), new_canister_id);

        let mut state = initial_state();
        state
            .upgrade(UpgradeArgs {
                manual_deposit_fee: Some(new_manual_fee),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(state.manual_deposit_fee(), new_manual_fee);

        let mut state = initial_state();
        state
            .upgrade(UpgradeArgs {
                automated_deposit_fee: Some(new_automated_fee),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(state.automated_deposit_fee(), new_automated_fee);

        let mut state = initial_state();
        state
            .upgrade(UpgradeArgs {
                minimum_deposit_amount: Some(new_minimum_deposit_amount),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(state.minimum_deposit_amount(), new_minimum_deposit_amount);

        let mut state = initial_state();
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

        let mut state = initial_state();
        state
            .upgrade(UpgradeArgs {
                withdrawal_fee: Some(new_withdrawal_fee),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(state.withdrawal_fee(), new_withdrawal_fee);

        let mut state = initial_state();
        state
            .upgrade(UpgradeArgs {
                process_deposit_required_cycles: Some(new_process_deposit_required_cycles),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(
            state.process_deposit_required_cycles(),
            new_process_deposit_required_cycles as u128
        );
    }

    #[test]
    fn should_fail_when_sol_rpc_canister_id_is_anonymous() {
        assert_matches!(
            initial_state().upgrade(UpgradeArgs {
                sol_rpc_canister_id: Some(Principal::anonymous()),
                ..Default::default()
            }),
            Err(InvalidStateError::InvalidCanisterId(_))
        );
    }

    // This test ensures the canister state is rolled back after a failed upgrade
    #[test]
    #[should_panic = "InvalidDepositFees"]
    fn should_panic_when_upgrade_fails() {
        let mut state = initial_state();
        let new_minimum_deposit_amount = AUTOMATED_DEPOSIT_FEE - 1;

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
        let signers: Vec<_> = (0..num_signers)
            .map(|i| account(100 + i as usize))
            .collect();
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
    let expected = DEPOSIT_1 + DEPOSIT_2 - 2 * FEE_PER_SIGNATURE;
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
    let expected = expected - TRANSFER_1 - TRANSFER_2 - FEE_PER_SIGNATURE;
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
