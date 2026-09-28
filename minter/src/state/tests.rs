use super::{event::*, *};
use crate::{
    constants::{FEE_PER_SIGNATURE, GET_TRANSACTION_CYCLES, RENT_EXEMPTION_THRESHOLD},
    rpc::BlockHeight,
    sol_transfer::MAX_SIGNATURES,
    state::{audit::process_event, read_state},
    test_fixtures::{
        AUTOMATED_DEPOSIT_FEE, DEPOSIT_CONSOLIDATION_FEE, MINIMUM_DEPOSIT_AMOUNT,
        MINIMUM_WITHDRAWAL_AMOUNT, PROCESS_DEPOSIT_REQUIRED_CYCLES, WITHDRAWAL_FEE, account,
        arb::arb_event,
        events::{
            accept_withdrawal, accept_withdrawal_at, credit_sweep, expire_transaction,
            fail_transaction, queue_deposit, resubmit_transaction, submit_sweep, submit_withdrawal,
            succeed_transaction,
        },
        init_balance, init_schnorr_master_key, init_state, ledger_canister_id, planned_sweep,
        queued_deposit,
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

mod cache_minter_public_key {
    use super::*;
    use ic_ed25519::{PocketIcMasterPublicKeyId, PublicKey};

    #[test]
    fn should_ignore_caching_the_same_key_again() {
        let mut state = state();
        let key = schnorr_public_key(1);

        state.cache_minter_public_key(key.clone());
        state.cache_minter_public_key(key.clone());

        assert_eq!(state.minter_public_key(), Some(&key));
    }

    #[test]
    #[should_panic(expected = "BUG: attempt to overwrite the minter public key")]
    fn should_panic_when_caching_a_different_key() {
        let mut state = state();

        state.cache_minter_public_key(schnorr_public_key(1));
        state.cache_minter_public_key(schnorr_public_key(2));
    }

    fn state() -> State {
        State::try_from(valid_init_args()).unwrap()
    }

    fn schnorr_public_key(chain_code_byte: u8) -> SchnorrPublicKey {
        SchnorrPublicKey {
            public_key: PublicKey::pocketic_key(PocketIcMasterPublicKeyId::Key1),
            chain_code: [chain_code_byte; 32],
        }
    }
}

mod minter_public_key_fetched {
    use super::*;
    use crate::{
        state::audit::replay_events,
        test_fixtures::{minter_public_key_fetched_event, schnorr_master_key},
    };

    #[test]
    fn should_make_the_key_available_after_replay() {
        let state = replay_events(vec![
            Event {
                timestamp: 0,
                payload: EventType::Init(valid_init_args()),
            },
            Event {
                timestamp: 1,
                payload: minter_public_key_fetched_event(),
            },
        ]);

        assert_eq!(state.minter_public_key(), Some(&schnorr_master_key()));
    }
}

mod queued_deposits {
    use super::*;
    use crate::state::audit::replay_events;

    #[test]
    fn should_replay_queued_deposits_like_direct_transitions() {
        let queued = |deposit_id| Event {
            timestamp: 0,
            payload: EventType::QueuedDeposit {
                deposit_id,
                account: queued_deposit(deposit_id).account,
                address: queued_deposit(deposit_id).address,
                balance: queued_deposit(deposit_id).balance,
            },
        };
        let init = Event {
            timestamp: 0,
            payload: EventType::Init(valid_init_args()),
        };
        let key_fetched = Event {
            timestamp: 0,
            payload: crate::test_fixtures::minter_public_key_fetched_event(),
        };
        let mut expected = State::try_from(valid_init_args()).unwrap();
        expected.cache_minter_public_key(crate::test_fixtures::schnorr_master_key());
        for deposit_id in 0..2 {
            let deposit = queued_deposit(deposit_id);
            expected.process_queued_deposit(
                deposit_id,
                &deposit.account,
                &deposit.address,
                deposit.balance,
            );
        }

        let replayed = replay_events([init, key_fetched, queued(0), queued(1)]);

        assert_eq!(replayed, expected);
    }
}

mod swept_deposits {
    use super::*;
    use crate::{
        state::{audit::replay_events, reset_state},
        storage::{reset_events, with_event_iter},
        test_fixtures::{
            DEFAULT_BLOCK_HEIGHT,
            events::{credit_sweep, quarantine_sweep, queue_deposits, submit_sweep},
            flow::deposit::{CreditedSweepFlow, DepositFlow, SweepFlow},
            sweep_message,
        },
    };

    const SWEEP_SIGNATURE_INDEX: usize = 0xAA;

    #[test]
    fn should_move_queued_deposits_to_swept_with_signature_and_received_amount() {
        init_state();
        init_schnorr_master_key();
        let [first, _, third] = queue_deposits();
        let sweep_signature = signature(SWEEP_SIGNATURE_INDEX);

        submit_sweep(sweep_signature, vec![2, 0]);

        read_state(|s| {
            assert_eq!(
                s.deposits().swept().get(&sweep_signature),
                Some(&planned_sweep([(0, first), (2, third)]))
            );
            assert_eq!(s.deposits().queued().keys().collect::<Vec<_>>(), vec![&1]);
            let transaction = s.submitted_transactions().get(&sweep_signature).unwrap();
            assert_eq!(
                transaction.amount,
                first.sweepable_amount() + third.sweepable_amount() - 2 * FEE_PER_SIGNATURE
            );
            assert_eq!(
                transaction.signers,
                vec![
                    Signer::Account(third.account),
                    Signer::Account(first.account)
                ]
            );
            assert_eq!(
                transaction.purpose,
                TransactionPurpose::SweepDeposits {
                    deposit_ids: vec![2, 0],
                }
            );
            assert_eq!(s.balance(), 0);
        });
    }

    #[test]
    fn should_finalize_the_sweep_when_the_transaction_succeeds() {
        init_state();
        init_schnorr_master_key();
        let [first, _, third] = queue_deposits();
        let sweep_signature = signature(SWEEP_SIGNATURE_INDEX);
        submit_sweep(sweep_signature, vec![2, 0]);

        succeed_transaction(sweep_signature);

        read_state(|s| {
            assert!(s.submitted_transactions().is_empty());
            assert!(s.deposits().swept().is_empty());
            assert_eq!(
                s.deposits().finalized().get(&sweep_signature),
                Some(&planned_sweep([(0, first), (2, third)]))
            );
            assert_eq!(s.balance(), 0);
        });
    }

    #[test]
    fn should_drop_swept_deposits_when_the_sweep_fails_or_expires() {
        type RecordOutcome = fn(Signature);
        let outcomes: [(&str, RecordOutcome); 2] = [
            ("failed", fail_transaction),
            ("expired", expire_transaction),
        ];
        for (outcome, record_outcome) in outcomes {
            reset_state();
            reset_events();
            init_state();
            init_schnorr_master_key();
            queue_deposits::<3>();
            let sweep_signature = signature(SWEEP_SIGNATURE_INDEX);
            submit_sweep(sweep_signature, vec![2, 0]);

            record_outcome(sweep_signature);

            read_state(|s| {
                assert!(s.submitted_transactions().is_empty(), "{outcome}");
                assert!(s.transactions_to_resubmit().is_empty(), "{outcome}");
                assert!(s.deposits().swept().is_empty(), "{outcome}");
                assert_eq!(s.deposits().dropped().len(), 2, "{outcome}");
                assert_eq!(s.balance(), 0, "{outcome}");
            });
        }
    }

    #[test]
    #[should_panic(expected = "must be dropped instead of resubmitted")]
    fn should_panic_when_resubmitting_a_sweep() {
        init_state();
        init_schnorr_master_key();
        queue_deposits::<3>();
        let sweep_signature = signature(SWEEP_SIGNATURE_INDEX);
        submit_sweep(sweep_signature, vec![2, 0]);
        mutate_state(|s| {
            let transaction = s.submitted_transactions.remove(&sweep_signature).unwrap();
            s.transactions_to_resubmit
                .insert(sweep_signature, transaction);
        });

        resubmit_transaction(sweep_signature, signature(SWEEP_SIGNATURE_INDEX + 1));
    }

    #[test]
    fn should_credit_the_balance_with_the_amount_received_by_the_sweep() {
        init_state();
        init_schnorr_master_key();
        let [first, _, third] = queue_deposits();
        let sweep_signature = signature(SWEEP_SIGNATURE_INDEX);
        submit_sweep(sweep_signature, vec![2, 0]);
        succeed_transaction(sweep_signature);
        let amount_received = planned_sweep([(0, first), (2, third)]).expected_received();

        credit_sweep(sweep_signature, amount_received);

        read_state(|s| {
            assert!(s.deposits().finalized().is_empty());
            assert_eq!(
                s.deposits().pending_mints().keys().collect::<Vec<_>>(),
                vec![&0, &2]
            );
            assert_eq!(s.balance(), amount_received);
        });
    }

    #[test]
    #[should_panic(expected = "exceeding the 399 lamports received")]
    fn should_panic_when_the_mints_exceed_the_amount_received() {
        init_state();
        init_schnorr_master_key();
        queue_deposits::<3>();
        let sweep_signature = signature(SWEEP_SIGNATURE_INDEX);
        submit_sweep(sweep_signature, vec![2, 0]);
        succeed_transaction(sweep_signature);

        mutate_state(|s| {
            process_event(
                s,
                EventType::CreditedSweep {
                    signature: sweep_signature,
                    amount_received: 399,
                    mints: vec![
                        CreditedDeposit {
                            deposit_id: 0,
                            amount_to_mint: 100,
                        },
                        CreditedDeposit {
                            deposit_id: 2,
                            amount_to_mint: 300,
                        },
                    ],
                },
                &TestCanisterRuntime::new().add_times([0, 0]),
            )
        });
    }

    #[test]
    fn should_mint_pending_deposit_without_changing_the_balance() {
        init_state();
        let credited = credit_sweep_of_two_deposits(0);
        let [to_mint, _] = credited.pending_mints();

        to_mint.mint(42);

        read_state(|s| assert_eq!(s.balance(), credited.amount_received));
    }

    #[test]
    fn should_quarantine_pending_mint_without_changing_the_balance() {
        init_state();
        let credited = credit_sweep_of_two_deposits(0);
        let [to_quarantine, _] = credited.pending_mints();

        to_quarantine.quarantine();

        read_state(|s| assert_eq!(s.balance(), credited.amount_received));
    }

    #[test]
    fn should_replay_the_recorded_events_including_created_at_time() {
        const CREDITED_AT: u64 = 1_234_000_000;
        init_state();
        let credited = credit_sweep_of_two_deposits(CREDITED_AT);
        let [to_mint, sibling] = credited.pending_mints();
        to_mint.mint(42);
        let recorded_events: Vec<Event> = with_event_iter(|events| events.collect());

        let replayed = replay_events(
            [
                Event {
                    timestamp: 0,
                    payload: EventType::Init(valid_init_args()),
                },
                Event {
                    timestamp: 0,
                    payload: crate::test_fixtures::minter_public_key_fetched_event(),
                },
            ]
            .into_iter()
            .chain(recorded_events),
        );

        assert_eq!(
            replayed.deposits().pending_mints()[&sibling.deposit_id].created_at_time,
            CREDITED_AT
        );
        read_state(|live_state| assert_eq!(&replayed, live_state));
    }

    fn credit_sweep_of_two_deposits(credited_at: u64) -> CreditedSweepFlow {
        init_schnorr_master_key();
        let deposits = [1, 2].map(|i| DepositFlow::queue(account(i), 1_000_000 * i as u64));
        SweepFlow::of(deposits)
            .submit(signature(SWEEP_SIGNATURE_INDEX))
            .succeed()
            .credit_at(credited_at)
    }

    #[test]
    fn should_quarantine_finalized_deposits_without_crediting_the_balance() {
        init_state();
        init_schnorr_master_key();
        queue_deposits::<3>();
        let sweep_signature = signature(SWEEP_SIGNATURE_INDEX);
        submit_sweep(sweep_signature, vec![2, 0]);
        succeed_transaction(sweep_signature);

        quarantine_sweep(sweep_signature);

        read_state(|s| {
            assert!(s.deposits().finalized().is_empty());
            assert_eq!(s.deposits().quarantined().len(), 2);
            assert_eq!(s.balance(), 0);
        });
    }

    #[test]
    #[should_panic(
        expected = "BUG: a sweep was submitted before the minter public key was recorded"
    )]
    fn should_panic_when_a_sweep_is_submitted_without_the_minter_public_key() {
        let mut state = State::try_from(valid_init_args()).unwrap();

        state.process_transaction_submitted(
            &signature(SWEEP_SIGNATURE_INDEX),
            &sweep_message([(0, queued_deposit(0))]),
            &[Signer::Account(queued_deposit(0).account)],
            &TransactionPurpose::SweepDeposits {
                deposit_ids: vec![0],
            },
            DEFAULT_BLOCK_HEIGHT,
        );
    }
}

mod state_validation {
    use super::*;

    #[test]
    fn should_fail_with_invalid_args() {
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
        // minimum_deposit_amount below the fee of a full sweep + rent exemption threshold
        // (automated_deposit_fee set to 1 to isolate this condition)
        let maximum_sweep_fee = MAX_SIGNATURES * FEE_PER_SIGNATURE;
        let minimum_required = maximum_sweep_fee + RENT_EXEMPTION_THRESHOLD;
        assert_fails_both(
            InitArgs {
                automated_deposit_fee: 1,
                minimum_deposit_amount: minimum_required - 1,
                ..valid_init_args()
            },
            UpgradeArgs {
                automated_deposit_fee: Some(1),
                minimum_deposit_amount: Some(minimum_required - 1),
                ..Default::default()
            },
            |e| {
                e == &InvalidStateError::InvalidMinimumDepositAmount {
                    minimum_deposit_amount: minimum_required - 1,
                    maximum_sweep_fee,
                    rent_exemption_threshold: RENT_EXEMPTION_THRESHOLD,
                }
            },
        );
        // minimum_deposit_amount leaves an empty main address below the rent exemption threshold
        let minimum_funding_main_address = 2 * RENT_EXEMPTION_THRESHOLD + FEE_PER_SIGNATURE;
        assert_fails_both(
            InitArgs {
                automated_deposit_fee: 1,
                minimum_deposit_amount: minimum_funding_main_address - 1,
                ..valid_init_args()
            },
            UpgradeArgs {
                automated_deposit_fee: Some(1),
                minimum_deposit_amount: Some(minimum_funding_main_address - 1),
                ..Default::default()
            },
            |e| {
                e == &InvalidStateError::MinimumDepositAmountLeavesMainAddressBelowRent {
                    minimum_deposit_amount: minimum_funding_main_address - 1,
                    rent_exemption_threshold: RENT_EXEMPTION_THRESHOLD,
                    fee_per_signature: FEE_PER_SIGNATURE,
                }
            },
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
        let minimum_required = GET_TRANSACTION_CYCLES + DEPOSIT_CONSOLIDATION_FEE;
        assert_fails_both(
            InitArgs {
                process_deposit_required_cycles: (minimum_required - 1) as u64,
                ..valid_init_args()
            },
            UpgradeArgs {
                process_deposit_required_cycles: Some((minimum_required - 1) as u64),
                ..Default::default()
            },
            |e| {
                e == &InvalidStateError::ProcessDepositRequiredCyclesTooLow {
                    required_cycles: minimum_required - 1,
                    get_transaction_cycles: GET_TRANSACTION_CYCLES,
                    consolidation_fee: DEPOSIT_CONSOLIDATION_FEE,
                }
            },
        );
        let maximum_fee = PROCESS_DEPOSIT_REQUIRED_CYCLES - GET_TRANSACTION_CYCLES;
        assert_fails_both(
            InitArgs {
                deposit_consolidation_fee: (maximum_fee + 1) as u64,
                ..valid_init_args()
            },
            UpgradeArgs {
                deposit_consolidation_fee: Some((maximum_fee + 1) as u64),
                ..Default::default()
            },
            |e| {
                e == &InvalidStateError::ProcessDepositRequiredCyclesTooLow {
                    required_cycles: PROCESS_DEPOSIT_REQUIRED_CYCLES,
                    get_transaction_cycles: GET_TRANSACTION_CYCLES,
                    consolidation_fee: maximum_fee + 1,
                }
            },
        );
    }

    #[test]
    fn should_succeed_at_boundary_conditions() {
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
        // minimum_deposit_amount can equal twice the rent exemption threshold + one signature fee
        let minimum_required = 2 * RENT_EXEMPTION_THRESHOLD + FEE_PER_SIGNATURE;
        assert_succeeds_both(
            InitArgs {
                automated_deposit_fee: 1,
                minimum_deposit_amount: minimum_required,
                ..valid_init_args()
            },
            UpgradeArgs {
                automated_deposit_fee: Some(1),
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
        let minimum_required = GET_TRANSACTION_CYCLES + DEPOSIT_CONSOLIDATION_FEE;
        assert_succeeds_both(
            InitArgs {
                process_deposit_required_cycles: minimum_required as u64,
                ..valid_init_args()
            },
            UpgradeArgs {
                process_deposit_required_cycles: Some(minimum_required as u64),
                ..Default::default()
            },
        );
        let maximum_fee = PROCESS_DEPOSIT_REQUIRED_CYCLES - GET_TRANSACTION_CYCLES;
        assert_succeeds_both(
            InitArgs {
                deposit_consolidation_fee: maximum_fee as u64,
                ..valid_init_args()
            },
            UpgradeArgs {
                deposit_consolidation_fee: Some(maximum_fee as u64),
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
                automated_deposit_fee: AUTOMATED_DEPOSIT_FEE,
                deposit_consolidation_fee: DEPOSIT_CONSOLIDATION_FEE,
                withdrawal_fee: WITHDRAWAL_FEE,
                minimum_withdrawal_amount: MINIMUM_WITHDRAWAL_AMOUNT,
                minimum_deposit_amount: MINIMUM_DEPOSIT_AMOUNT,
                process_deposit_required_cycles: PROCESS_DEPOSIT_REQUIRED_CYCLES,
                pending_deposit_sol_request_guards: BTreeSet::new(),
                pending_withdrawal_request_guards: BTreeSet::new(),
                deposits: Deposits::default(),
                pending_withdrawal_requests: BTreeMap::new(),
                sent_withdrawal_requests: BTreeMap::new(),
                successful_withdrawal_requests: BTreeMap::new(),
                failed_withdrawal_requests: BTreeMap::new(),
                submitted_transactions: InsertionOrderedMap::new(),
                transactions_to_resubmit: InsertionOrderedMap::new(),
                succeeded_transactions: BTreeSet::new(),
                failed_transactions: InsertionOrderedMap::new(),
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
            &TestCanisterRuntime::new().add_times([0, 0]),
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
            .map(|i| Signer::Account(account(100 + i as usize)))
            .collect();
        mutate_state(|state| {
            process_event(
                state,
                EventType::SubmittedTransaction {
                    signature: sig,
                    message: message_with_signers(num_signers).into(),
                    signers,
                    purpose,
                    block_height: BlockHeight::new(0),
                },
                &TestCanisterRuntime::new().add_times([0, 0]),
            )
        });
    }

    init_state();
    assert_eq!(read_state(|s| s.balance()), 0);

    // Queueing and sweeping deposits does not change the balance
    queue_deposit(0, account(1), DEPOSIT_1);
    queue_deposit(1, account(2), DEPOSIT_2);
    submit_sweep(signature(0xAA), vec![0, 1]);
    assert_eq!(read_state(|s| s.balance()), 0);

    // A finalized sweep does not change the balance until it is credited
    succeed_transaction(signature(0xAA));
    assert_eq!(read_state(|s| s.balance()), 0);

    // Crediting the sweep adds the amount received: balance += total_deposits - tx_fee(2 signers)
    let expected = DEPOSIT_1 + DEPOSIT_2 - 2 * FEE_PER_SIGNATURE;
    credit_sweep(signature(0xAA), expected);
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

    // A failed sweep does not credit the balance
    queue_deposit(2, account(5), DEPOSIT_3);
    submit_sweep(signature(0xCC), vec![2]);
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

        submit_withdrawal(signature(0xAA), vec![0, 1]);

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

        submit_withdrawal(signature(0xAA), vec![0]);
        submit_withdrawal(signature(0xBB), vec![1]);
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

        submit_withdrawal(signature(0xAA), vec![0]);
        submit_withdrawal(signature(0xBB), vec![1]);
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

        submit_withdrawal(signature(0xAA), vec![0, 1]);
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

        submit_withdrawal(signature(0xAA), vec![0, 1]);

        assert_eq!(
            read_state(|s| s.oldest_incomplete_withdrawal_created_at()),
            Some(1_000_000_000)
        );

        // Expire then resubmit the transaction with a new signature
        expire_transaction(signature(0xAA));
        resubmit_transaction(signature(0xAA), signature(0xBB));

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

mod withdrawal_batches {
    use super::*;
    use crate::sol_transfer::{BATCH_WITHDRAWAL_TX_FEE, MAX_WITHDRAWALS_PER_TX};

    const MAX_AMOUNT_TO_TRANSFER: Lamport = u64::MAX - WITHDRAWAL_FEE - BATCH_WITHDRAWAL_TX_FEE;
    const NUM_REQUESTS_FOR_TWO_BATCHES: usize = MAX_WITHDRAWALS_PER_TX + 1;
    const COST_OF_TWO_BATCHES: Lamport = NUM_REQUESTS_FOR_TWO_BATCHES as u64
        * MINIMUM_WITHDRAWAL_AMOUNT
        + 2 * BATCH_WITHDRAWAL_TX_FEE;

    #[test]
    fn should_be_empty_when_no_pending_withdrawals() {
        let state = state();

        assert_eq!(state.withdrawal_batches().next(), None);
    }

    proptest! {
        #[test]
        fn should_batch_single_request_when_balance_covers_amount_and_fee(
            amount_to_transfer in MINIMUM_WITHDRAWAL_AMOUNT..=MAX_AMOUNT_TO_TRANSFER
        ) {
            let mut state = state();
            state.balance = amount_to_transfer + BATCH_WITHDRAWAL_TX_FEE;
            let requests = [withdrawal_request(0, amount_to_transfer)];
            accept_withdrawal_requests(&mut state, requests.clone());

            let batches: Vec<_> = state.withdrawal_batches().collect();

            prop_assert_eq!(batches, vec![vec![requests[0].clone()]]);
        }

        #[test]
        fn should_be_empty_when_balance_does_not_cover_amount_and_fee(
            amount_to_transfer in MINIMUM_WITHDRAWAL_AMOUNT..=MAX_AMOUNT_TO_TRANSFER,
            shortfall in 1..=MINIMUM_WITHDRAWAL_AMOUNT + BATCH_WITHDRAWAL_TX_FEE
        ) {
            let mut state = state();
            state.balance = amount_to_transfer + BATCH_WITHDRAWAL_TX_FEE - shortfall;
            let requests = [withdrawal_request(0, amount_to_transfer)];
            accept_withdrawal_requests(&mut state, requests);

            prop_assert_eq!(state.withdrawal_batches().next(), None);
        }

        #[test]
        fn should_split_requests_into_batches_of_max_size_when_balance_covers_both_fees(
            balance in COST_OF_TWO_BATCHES..=u64::MAX
        ) {
            let mut state = state();
            state.balance = balance;
            let requests = withdrawal_requests(NUM_REQUESTS_FOR_TWO_BATCHES);
            accept_withdrawal_requests(&mut state, requests.clone());

            let batches: Vec<_> = state.withdrawal_batches().collect();

            prop_assert_eq!(
                batches,
                vec![
                    requests[..MAX_WITHDRAWALS_PER_TX].to_vec(),
                    requests[MAX_WITHDRAWALS_PER_TX..].to_vec()
                ]
            );
        }

        #[test]
        fn should_not_start_second_batch_when_balance_does_not_cover_its_amount_and_fee(
            shortfall in 1..=MINIMUM_WITHDRAWAL_AMOUNT + BATCH_WITHDRAWAL_TX_FEE
        ) {
            let mut state = state();
            state.balance = COST_OF_TWO_BATCHES - shortfall;
            let requests = withdrawal_requests(NUM_REQUESTS_FOR_TWO_BATCHES);
            accept_withdrawal_requests(&mut state, requests.clone());

            let batches: Vec<_> = state.withdrawal_batches().collect();

            prop_assert_eq!(batches, vec![requests[..MAX_WITHDRAWALS_PER_TX].to_vec()]);
        }
    }

    #[test]
    fn should_be_empty_when_cost_overflows() {
        let mut state = state();
        state.balance = u64::MAX;
        let request = WithdrawalRequest {
            amount_to_transfer: u64::MAX,
            burned_amount: u64::MAX,
            ..withdrawal_request(0, MINIMUM_WITHDRAWAL_AMOUNT)
        };
        accept_withdrawal_requests(&mut state, [request]);

        assert_eq!(state.withdrawal_batches().next(), None);
    }

    #[test]
    fn should_stop_at_first_unaffordable_request_without_skipping_it() {
        let mut state = state();
        state.balance = 2 * MINIMUM_WITHDRAWAL_AMOUNT + BATCH_WITHDRAWAL_TX_FEE;
        let requests = [
            withdrawal_request(0, MINIMUM_WITHDRAWAL_AMOUNT),
            withdrawal_request(1, 2 * MINIMUM_WITHDRAWAL_AMOUNT),
            withdrawal_request(2, MINIMUM_WITHDRAWAL_AMOUNT),
        ];
        accept_withdrawal_requests(&mut state, requests.clone());

        let batches: Vec<_> = state.withdrawal_batches().collect();

        assert_eq!(batches, vec![vec![requests[0].clone()]]);
    }

    fn state() -> State {
        State::try_from(valid_init_args()).unwrap()
    }

    fn accept_withdrawal_requests<I: IntoIterator<Item = WithdrawalRequest>>(
        state: &mut State,
        requests: I,
    ) {
        for (index, request) in requests.into_iter().enumerate() {
            state.process_accepted_withdrawal(&request, index as u64);
        }
    }

    fn withdrawal_requests(count: usize) -> Vec<WithdrawalRequest> {
        (0..count)
            .map(|burn_index| withdrawal_request(burn_index, MINIMUM_WITHDRAWAL_AMOUNT))
            .collect()
    }

    fn withdrawal_request(burn_index: usize, amount_to_transfer: Lamport) -> WithdrawalRequest {
        WithdrawalRequest {
            account: account(burn_index),
            solana_address: [0u8; 32],
            burn_block_index: LedgerBurnIndex::from(burn_index as u64),
            amount_to_transfer,
            burned_amount: amount_to_transfer + WITHDRAWAL_FEE,
        }
    }
}
