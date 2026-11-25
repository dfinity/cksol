use super::*;
use crate::{
    constants::RENT_EXEMPTION_THRESHOLD,
    state::{
        State, Sweep,
        audit::{process_event, replay_events},
        event::{Event, EventType, TransactionPurpose},
        mutate_state, reset_state,
    },
    storage::{reset_events, with_event_iter},
    test_fixtures::{
        MINTER_ADDRESS, account, init_args_with_nonce_account, init_balance_to, init_state,
        minter_public_key_fetched_event, queued_deposit_of, runtime::TestCanisterRuntime,
        schnorr_master_key, signature, valid_init_args,
    },
};
use cksol_types_internal::{
    SplTokenRegistration, UpgradeArgs, event::TransactionPurpose as CandidPurpose,
};
use solana_signature::Signature;
use std::panic::{AssertUnwindSafe, catch_unwind};

fn sweep() -> SplSweep {
    let mint = Address::from([2; 32]);
    let account = account(1);
    let owner = account_address(&schnorr_master_key(), &account);
    let deposit = QueuedSplDeposit {
        account,
        mint,
        address: associated_token_address(&owner, &mint),
        balance: 1_234,
        fee: 34,
    };
    let token = SupportedSplToken {
        mint,
        token_program: spl_token_interface::id().to_bytes().into(),
        decimals: 6,
        ledger_id: candid::Principal::from_slice(&[99, 1]),
        index_id: None,
        symbol: "ckTest".into(),
        minimum_deposit_amount: 1,
        deposit_fee: 34,
        deposits_paused: false,
        withdrawal_fee_token: 0,
    };
    SplSweep::plan(42, deposit, &token, &schnorr_master_key()).unwrap()
}

#[test]
fn should_build_one_gross_transfer_with_the_minter_paying_for_two_signatures() {
    let sweep = sweep();
    let blockhash = Hash::new_from_array([7; 32]);
    let message = sweep.sweep_message(blockhash);
    assert_eq!(message.recent_blockhash, blockhash);
    assert_eq!(message.header.num_required_signatures, 2);
    assert_eq!(message.account_keys[0], MINTER_ADDRESS);
    assert_eq!(message.account_keys[1], sweep.owner);
    assert_eq!(
        sweep.signers(),
        [Signer::Minter, Signer::Account(account(1))]
    );
    assert_eq!(message.instructions.len(), 1);
    let transfer = &message.instructions[0];
    assert_eq!(
        message.account_keys[transfer.program_id_index as usize],
        spl_token_interface::id()
    );
    let keys: Vec<_> = transfer
        .accounts
        .iter()
        .map(|i| message.account_keys[*i as usize])
        .collect();
    assert_eq!(
        keys,
        vec![
            sweep.deposit.address,
            sweep.deposit.mint,
            associated_token_address(&MINTER_ADDRESS, &sweep.deposit.mint),
            sweep.owner
        ]
    );
    assert_eq!(
        transfer.data,
        [vec![12], 1_234u64.to_le_bytes().to_vec(), vec![6]].concat()
    );
    let transaction = solana_transaction::Transaction::new_unsigned(message);
    assert!(bincode::serialize(&transaction).unwrap().len() <= crate::sol_transfer::MAX_TX_SIZE);
    assert_eq!(SPL_SWEEP_TRANSACTION_FEE, 10_000);
}

#[test]
fn should_recover_only_the_captured_single_deposit_message_and_ordered_signers() {
    let sweep = sweep();
    let token = SupportedSplToken {
        mint: sweep.deposit.mint,
        token_program: spl_token_interface::id().to_bytes().into(),
        decimals: 6,
        ledger_id: candid::Principal::from_slice(&[99, 1]),
        index_id: None,
        symbol: "ckTest".into(),
        // Current acceptance settings do not change captured terms.
        minimum_deposit_amount: 9_999,
        deposit_fee: 5_000,
        deposits_paused: true,
        withdrawal_fee_token: 0,
    };
    let message = sweep.sweep_message(Hash::default());
    let recover = |message: Message, signers: &[Signer]| {
        SplSweep::recover(
            42,
            sweep.deposit.clone(),
            &token,
            &schnorr_master_key(),
            &VersionedMessage::Legacy(message),
            signers,
        )
    };
    assert_eq!(
        recover(message.clone(), &sweep.signers()),
        Ok(sweep.clone())
    );
    for signers in [
        vec![],
        vec![Signer::Minter],
        vec![Signer::Account(account(1)), Signer::Minter],
    ] {
        assert_eq!(
            recover(message.clone(), &signers),
            Err(SplSweepRecoveryError::InvalidSigners)
        );
    }
    let mut wrong_amount = message.clone();
    wrong_amount.instructions[0].data[1] ^= 1;
    let mut wrong_decimals = message.clone();
    wrong_decimals.instructions[0].data[9] = 9;
    let mut extra_instruction = message.clone();
    extra_instruction
        .instructions
        .push(message.instructions[0].clone());
    let mut wrong_payer = message.clone();
    wrong_payer.account_keys[0] = [9; 32].into();
    let mut wrong_destination = message.clone();
    let destination_index = message.instructions[0].accounts[2] as usize;
    wrong_destination.account_keys[destination_index] = [8; 32].into();
    for invalid in [
        wrong_amount,
        wrong_decimals,
        extra_instruction,
        wrong_payer,
        wrong_destination,
    ] {
        assert!(matches!(
            recover(invalid, &sweep.signers()),
            Err(SplSweepRecoveryError::UnexpectedMessage { .. })
        ));
    }
    let mut wrong_source = sweep.deposit.clone();
    wrong_source.address = [7; 32].into();
    assert_eq!(
        SplSweep::plan(42, wrong_source, &token, &schnorr_master_key()),
        Err(SplSweepRecoveryError::SourceMismatch { deposit_id: 42 })
    );
    let mut wrong_mint = token.clone();
    wrong_mint.mint = Address::from([8; 32]);
    assert_eq!(
        SplSweep::plan(
            42,
            sweep.deposit.clone(),
            &wrong_mint,
            &schnorr_master_key()
        ),
        Err(SplSweepRecoveryError::MintMismatch)
    );
    let mut wrong_token = token;
    wrong_token.token_program = Address::from([9; 32]);
    assert_eq!(
        SplSweep::plan(
            42,
            sweep.deposit.clone(),
            &wrong_token,
            &schnorr_master_key()
        ),
        Err(SplSweepRecoveryError::MintMismatch)
    );
}

#[test]
fn should_preserve_the_scalar_spl_purpose_tag_and_candid_roundtrip() {
    let purpose = TransactionPurpose::SweepSplDeposit {
        deposit_id: 42,
        block_height: 100.into(),
    };
    assert_eq!(
        minicbor::to_vec(&purpose).unwrap(),
        vec![0x82, 5, 0x82, 0x18, 42, 0x18, 100]
    );
    let encoded = minicbor::to_vec(&purpose).unwrap();
    assert_eq!(
        minicbor::decode::<TransactionPurpose>(&encoded).unwrap(),
        purpose
    );
    let wire = candid::encode_one(CandidPurpose::from(purpose.clone())).unwrap();
    let public: CandidPurpose = candid::decode_one(&wire).unwrap();
    assert_eq!(TransactionPurpose::from(public), purpose);
}

#[test]
fn should_reject_invalid_submission_events_before_reserving_or_removing_the_deposit() {
    let mut sweep = sweep();
    sweep.deposit_id = 0;
    let mut state = State::try_from(valid_init_args()).unwrap();
    state.cache_minter_public_key(schnorr_master_key());
    state
        .upgrade(UpgradeArgs {
            add_spl_tokens: Some(vec![SplTokenRegistration {
                mint: sweep.deposit.mint.to_string(),
                token_program: spl_token_interface::id().to_string(),
                decimals: 6,
                ledger_id: candid::Principal::from_slice(&[99, 1]),
                index_id: None,
                symbol: "ckTest".into(),
                minimum_deposit_amount: 1,
                deposit_fee: 34,
                deposits_paused: false,
                withdrawal_fee_token: None,
            }]),
            ..Default::default()
        })
        .unwrap();
    state.spl_deposits.queue(0, sweep.deposit.clone(), 42);
    state.balance = RENT_EXEMPTION_THRESHOLD + 20_000;
    state.operating_balance.request_refill(0, 20_000, 100);
    state.operating_balance.complete_refill(0);
    let purpose = TransactionPurpose::SweepSplDeposit {
        deposit_id: 0,
        block_height: 100.into(),
    };
    let valid = sweep.sweep_message(Hash::default());
    let mut invalid = valid.clone();
    invalid.instructions[0].data[1] ^= 1;
    for (message, signers) in [
        (invalid, sweep.signers().to_vec()),
        (valid.clone(), vec![Signer::Minter]),
    ] {
        let before = state.clone();
        assert!(
            catch_unwind(AssertUnwindSafe(|| state.process_transaction_submitted(
                &signature(1),
                &message.into(),
                &signers,
                &purpose,
                100,
            )))
            .is_err()
        );
        assert_eq!(state, before);
    }
    state.process_transaction_submitted(
        &signature(1),
        &valid.clone().into(),
        &sweep.signers(),
        &purpose,
        100,
    );
    let before = state.clone();
    // A fresh signature cannot submit the same captured deposit twice.
    assert!(
        catch_unwind(AssertUnwindSafe(|| state.process_transaction_submitted(
            &signature(2),
            &valid.into(),
            &sweep.signers(),
            &purpose,
            100,
        )))
        .is_err()
    );
    assert_eq!(state, before);
    assert_eq!(state.balance(), RENT_EXEMPTION_THRESHOLD + 10_000);
    assert_eq!(state.operating_balance().balance(), 10_000);
    assert_eq!(
        state
            .spl_deposits()
            .in_flight_id(&account(1), &sweep.deposit.mint),
        Some(0)
    );
}

/// Builds a valid event log, including SOL funding, without assigning balances by hand.
fn submission_log(main_lamports: u64, operating_lamports: u64) -> (SplSweep, Vec<Event>) {
    reset_state();
    reset_events();
    init_state();
    init_balance_to(main_lamports);
    let mut sweep = sweep();
    sweep.deposit_id = 0;
    let runtime = TestCanisterRuntime::new().add_times([100; 8]);
    let payloads = [
        EventType::Upgrade(UpgradeArgs {
            add_spl_tokens: Some(vec![SplTokenRegistration {
                mint: sweep.deposit.mint.to_string(),
                token_program: spl_token_interface::id().to_string(),
                decimals: 6,
                ledger_id: candid::Principal::from_slice(&[99, 1]),
                index_id: None,
                symbol: "ckTest".into(),
                minimum_deposit_amount: 1,
                deposit_fee: 34,
                deposits_paused: false,
                withdrawal_fee_token: None,
            }]),
            ..Default::default()
        }),
        EventType::RequestedOperatingRefill {
            refill_id: 0,
            amount: operating_lamports,
            created_at_time: 100,
        },
        EventType::CompletedOperatingRefill {
            refill_id: 0,
            burn_block_index: 42,
        },
        EventType::QueuedSplDeposit {
            deposit_id: 0,
            account: sweep.deposit.account,
            mint: sweep.deposit.mint,
            address: sweep.deposit.address,
            balance: sweep.deposit.balance,
            fee: sweep.deposit.fee,
        },
    ];
    for payload in payloads {
        mutate_state(|state| process_event(state, payload, &runtime));
    }
    let mut events = vec![
        Event {
            timestamp: 0,
            payload: EventType::Init(init_args_with_nonce_account()),
        },
        Event {
            timestamp: 0,
            payload: minter_public_key_fetched_event(),
        },
    ];
    events.extend(with_event_iter(|events| events.collect::<Vec<_>>()));
    (sweep, events)
}

fn spl_submission(sweep: &SplSweep, signature: Signature) -> EventType {
    EventType::SubmittedTransaction {
        signature,
        message: sweep.sweep_message(Hash::default()).into(),
        signers: sweep.signers().to_vec(),
        purpose: TransactionPurpose::SweepSplDeposit {
            deposit_id: sweep.deposit_id(),
            block_height: 100.into(),
        },
    }
}

/// Queues a valid SOL deposit and returns its otherwise valid submission event.
fn sol_submission(events: &mut Vec<Event>, signature: Signature) -> EventType {
    let state = replay_events(events.iter().map(|event| Event {
        timestamp: event.timestamp,
        payload: event.payload.clone(),
    }));
    let deposit_id = state.deposits().next_id();
    let deposit = queued_deposit_of(account(7), 1_000_000);
    events.push(Event {
        timestamp: 100,
        payload: EventType::QueuedDeposit {
            deposit_id,
            account: deposit.account,
            address: deposit.address,
            balance: deposit.balance,
            deposit_fee: deposit.deposit_fee,
            origin: deposit.origin,
        },
    });
    EventType::SubmittedTransaction {
        signature,
        message: Sweep::plan(vec![(deposit_id, deposit)], MINTER_ADDRESS)
            .sweep_message(Hash::default())
            .into(),
        signers: vec![Signer::Account(deposit.account)],
        purpose: TransactionPurpose::SweepDeposit {
            deposit_ids: vec![deposit_id],
            block_height: 100.into(),
        },
    }
}

/// Whether a rejected event leaves the in-memory state untouched. Either way a canister
/// trap discards the changes of the message.
#[derive(Clone, Copy, PartialEq)]
enum Rejection {
    BeforeAnyChange,
    Trap,
}

fn assert_submission_rejected(
    events: Vec<Event>,
    rejected: EventType,
    expected: &str,
    rejection: Rejection,
) {
    fn assert_reason(error: Box<dyn std::any::Any + Send>, expected: &str) {
        let message = error
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| error.downcast_ref::<&str>().copied())
            .expect("rejection should carry a panic message");
        assert!(
            message.contains(expected),
            "expected {expected}, got {message}"
        );
    }
    let mut state = replay_events(events.iter().map(|event| Event {
        timestamp: event.timestamp,
        payload: event.payload.clone(),
    }));
    let before = state.clone();
    let recorded_before = with_event_iter(|events| events.count());
    let runtime = TestCanisterRuntime::new().add_times([100, 100]);
    let error = catch_unwind(AssertUnwindSafe(|| {
        process_event(&mut state, rejected.clone(), &runtime)
    }))
    .expect_err("invalid submission must be rejected");
    assert_reason(error, expected);
    if rejection == Rejection::BeforeAnyChange {
        assert_eq!(state, before, "rejection must precede all state changes");
    }
    assert_eq!(
        with_event_iter(|events| events.count()),
        recorded_before,
        "rejected events must not be recorded"
    );
    // Exercise the same rejection through a complete replay, rather than only a direct state call.
    let error = catch_unwind(AssertUnwindSafe(|| {
        replay_events(events.into_iter().chain(std::iter::once(Event {
            timestamp: 100,
            payload: rejected,
        })))
    }))
    .expect_err("replay must reject the invalid submission too");
    assert_reason(error, expected);
}

#[test]
fn should_reject_unfunded_submission_events_in_live_processing_and_replay() {
    let rent = RENT_EXEMPTION_THRESHOLD;
    for (main, operating) in [(rent + 20_000, 9_999), (rent + 9_999, 20_000)] {
        let (sweep, events) = submission_log(main, operating);
        let submission = spl_submission(&sweep, signature(10));
        assert_submission_rejected(
            events,
            submission,
            "Attempted to submit an unfunded SPL sweep",
            Rejection::BeforeAnyChange,
        );
    }
}

#[test]
fn should_reject_an_spl_submission_reusing_a_pending_sol_signature() {
    let (sweep, mut events) = submission_log(RENT_EXEMPTION_THRESHOLD + 20_000, 20_000);
    let signature = signature(10);
    let sol = sol_submission(&mut events, signature);
    events.push(Event {
        timestamp: 100,
        payload: sol,
    });
    let submission = spl_submission(&sweep, signature);
    assert_submission_rejected(
        events,
        submission,
        &format!("Attempted to submit transaction with signature {signature:?} twice"),
        Rejection::BeforeAnyChange,
    );
}

#[test]
fn should_reject_a_sol_submission_reusing_a_pending_spl_signature() {
    let (sweep, mut events) = submission_log(RENT_EXEMPTION_THRESHOLD + 20_000, 20_000);
    let signature = signature(10);
    events.push(Event {
        timestamp: 100,
        payload: spl_submission(&sweep, signature),
    });
    let sol = sol_submission(&mut events, signature);
    assert_submission_rejected(
        events,
        sol,
        // The SOL transition records the sweep of the deposit before this check, as upstream.
        &format!("Attempted to submit transaction with signature {signature:?} twice"),
        Rejection::Trap,
    );
}

mod settlement {
    use super::super::{SPL_SWEEP_TRANSACTION_FEE, SettledSplSweep};
    use crate::state::{SweepMismatch, SweepSettlementError, UnreadableOutcome};
    use crate::test_fixtures::spl_sweep as fixture;
    use solana_transaction::TransactionError;
    use solana_transaction_status_client_types::option_serializer::OptionSerializer;

    #[test]
    fn should_settle_only_the_captured_gross_transfer_and_actual_fee() {
        let sweep = fixture::plan(0);
        let transaction = fixture::transaction(&sweep);
        assert!(transaction.verify_with_results().iter().all(|valid| *valid));
        for fee in [0, 5_000, 10_000] {
            let mut meta = fixture::meta(&sweep);
            meta.fee = fee;
            meta.post_balances[0] = meta.pre_balances[0] - fee;
            assert_eq!(
                sweep.settle(&transaction, Some(&meta)),
                Ok(SettledSplSweep {
                    lamports_spent: fee,
                    failed: false
                })
            );
        }
    }

    #[test]
    fn should_validate_failed_transaction_cost_without_requiring_a_treasury_token_balance() {
        let sweep = fixture::plan(0);
        let transaction = fixture::transaction(&sweep);
        let mut meta = fixture::meta(&sweep);
        meta.err = Some(TransactionError::AccountNotFound.into());
        meta.pre_token_balances = OptionSerializer::None;
        meta.post_token_balances = OptionSerializer::None;
        assert_eq!(
            sweep.settle(&transaction, Some(&meta)),
            Ok(SettledSplSweep {
                lamports_spent: 10_000,
                failed: true
            })
        );
        meta.post_balances[1] -= 1;
        assert!(matches!(
            sweep.settle(&transaction, Some(&meta)),
            Err(SweepSettlementError::Mismatch(
                SweepMismatch::UnexpectedBalanceChange { .. }
            ))
        ));
    }

    #[test]
    fn should_distinguish_incomplete_metadata_from_contradictory_results() {
        let sweep = fixture::plan(0);
        let transaction = fixture::transaction(&sweep);
        let meta = fixture::meta(&sweep);
        assert_eq!(
            sweep.settle(&transaction, None),
            Err(UnreadableOutcome::NoMetaField.into())
        );
        let mut incomplete = meta.clone();
        incomplete.post_balances.pop();
        assert_eq!(
            sweep.settle(&transaction, Some(&incomplete)),
            Err(UnreadableOutcome::IncompleteBalances.into())
        );
        let mut excessive_fee = meta.clone();
        excessive_fee.fee = 10_001;
        assert_eq!(
            sweep.settle(&transaction, Some(&excessive_fee)),
            Err(SweepMismatch::UnexpectedFee {
                expected: SPL_SWEEP_TRANSACTION_FEE,
                actual: 10_001,
            }
            .into())
        );
        for index in 0..meta.post_balances.len() {
            let mut changed = meta.clone();
            changed.post_balances[index] -= 1;
            let keys = transaction.message.static_account_keys();
            assert_eq!(
                sweep.settle(&transaction, Some(&changed)),
                Err(SweepMismatch::UnexpectedBalanceChange {
                    address: keys[index],
                    pre: meta.pre_balances[index],
                    post: changed.post_balances[index],
                    expected_decrease: if index == 0 { meta.fee } else { 0 },
                }
                .into())
            );
        }
        let mut changed = transaction.clone();
        let solana_message::VersionedMessage::Legacy(message) = &mut changed.message else {
            unreachable!()
        };
        message.instructions[0].data[1] ^= 1;
        assert_eq!(
            sweep.settle(&changed, Some(&meta)),
            Err(SweepMismatch::UnexpectedMessage.into())
        );
    }

    #[test]
    fn should_require_complete_unique_pre_and_post_token_balances() {
        let sweep = fixture::plan(0);
        let transaction = fixture::transaction(&sweep);
        for pre in [true, false] {
            for case in 0..5 {
                let mut meta = fixture::meta(&sweep);
                let balances = if pre {
                    &mut meta.pre_token_balances
                } else {
                    &mut meta.post_token_balances
                };
                if case == 0 {
                    *balances = OptionSerializer::None;
                } else {
                    let OptionSerializer::Some(entries) = balances else {
                        unreachable!()
                    };
                    match case {
                        1 => {
                            entries.remove(0);
                        }
                        2 => {
                            entries.pop();
                        }
                        3 => {
                            entries.push(entries[0].clone());
                        }
                        4 => {
                            entries[0].account_index = 255;
                        }
                        _ => unreachable!(),
                    }
                }
                // Missing balances are incomplete. A duplicate or an unknown index is invalid.
                let result = sweep.settle(&transaction, Some(&meta));
                if case < 3 {
                    assert_eq!(
                        result,
                        Err(UnreadableOutcome::IncompleteTokenBalances.into()),
                        "pre={pre}, case={case}"
                    );
                } else {
                    assert!(
                        matches!(
                            result,
                            Err(SweepSettlementError::Unreadable(
                                UnreadableOutcome::InvalidTokenBalances { .. }
                            ))
                        ),
                        "pre={pre}, case={case}"
                    );
                }
            }
        }
    }

    #[test]
    fn should_validate_identity_amount_and_deltas_on_both_sides_of_the_transfer() {
        let sweep = fixture::plan(0);
        let transaction = fixture::transaction(&sweep);
        for pre in [true, false] {
            for index in 0..2 {
                for case in 0..8 {
                    let mut meta = fixture::meta(&sweep);
                    let balances = if pre {
                        &mut meta.pre_token_balances
                    } else {
                        &mut meta.post_token_balances
                    };
                    let OptionSerializer::Some(entries) = balances else {
                        unreachable!()
                    };
                    let entry = &mut entries[index];
                    match case {
                        0 => entry.mint = "wrong mint".into(),
                        1 => entry.owner = OptionSerializer::Some("wrong owner".into()),
                        2 => entry.program_id = OptionSerializer::Some("wrong program".into()),
                        3 => entry.ui_token_amount.decimals += 1,
                        4 => {
                            entry.ui_token_amount.amount =
                                (entry.ui_token_amount.amount.parse::<u64>().unwrap() + 1)
                                    .to_string()
                        }
                        5 => entry.owner = OptionSerializer::None,
                        6 => entry.program_id = OptionSerializer::Skip,
                        7 => entry.ui_token_amount.amount = "18446744073709551616".into(),
                        _ => unreachable!(),
                    }
                    let result = sweep.settle(&transaction, Some(&meta));
                    let context = format!("pre={pre}, index={index}, case={case}");
                    match case {
                        0..=3 => assert!(
                            matches!(
                                result,
                                Err(SweepSettlementError::Mismatch(
                                    SweepMismatch::UnexpectedTokenAccount { .. }
                                ))
                            ),
                            "{context}"
                        ),
                        4 => assert!(
                            matches!(
                                result,
                                Err(SweepSettlementError::Mismatch(
                                    SweepMismatch::UnexpectedTokenBalanceChange { .. }
                                ))
                            ),
                            "{context}"
                        ),
                        5 | 6 => assert_eq!(
                            result,
                            Err(UnreadableOutcome::IncompleteTokenBalances.into()),
                            "{context}"
                        ),
                        7 => assert!(
                            matches!(
                                result,
                                Err(SweepSettlementError::Unreadable(
                                    UnreadableOutcome::InvalidTokenBalances { .. }
                                ))
                            ),
                            "{context}"
                        ),
                        _ => unreachable!(),
                    }
                }
            }
        }
    }
}
