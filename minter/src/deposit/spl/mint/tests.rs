use super::process_pending_spl_mints;
use crate::deposit::spl::{deposit_spl, deposit_spl_status};
use crate::{
    address::{account_address, associated_token_address},
    constants::{LEDGER_DEDUPLICATION_WINDOW, MAX_PENDING_MINTS_PER_ROUND},
    deposit::sweep::process_pending_mints,
    state::{
        QueuedSplDeposit, SplQuarantineCause, SplSweep, State, TaskType,
        audit::process_event,
        event::{Event, EventType, TransactionPurpose},
        mutate_state, read_state, reset_state,
    },
    storage::{FailedMintReason, failed_mint_attempt_count, reset_events, with_event_iter},
    test_fixtures::{
        BLOCK_INDEX, DEFAULT_BLOCK_HEIGHT, EventsAssert, account, flow::deposit::DepositFlow,
        init_balance_to, init_state, replayed_recorded_log, runtime::TestCanisterRuntime,
        schnorr_master_key, signature,
    },
};
use candid::{Nat, Principal};
use cksol_types::{CollectedSplDepositFees, DepositSplError, DepositSplStatus, Memo, MintMemo};
use cksol_types_internal::{
    SplTokenRegistration, SplTokenUpdate, UpgradeArgs, event::Event as CandidEvent,
};
use futures::FutureExt;
use ic_canister_runtime::IcError;
use ic_stable_structures::Storable;
use icrc_ledger_types::icrc1::transfer::{BlockIndex, NumTokens, TransferArg, TransferError};
use solana_address::Address;
use solana_signature::Signature;
use std::panic::{AssertUnwindSafe, catch_unwind};

type MintResult = Result<BlockIndex, TransferError>;

const CREDITED_AT_TIME: u64 = 1_234;
const TOKEN_FEE: u64 = 10;

fn mint_a() -> Address {
    [2; 32].into()
}
fn mint_b() -> Address {
    [3; 32].into()
}
fn ledger_of(mint: Address) -> Principal {
    Principal::from_slice(&[mint.to_bytes()[0], 1])
}
fn sweep_signature(deposit_id: u64) -> Signature {
    signature(0x500 + deposit_id as usize)
}
fn record_at(time: u64, payload: EventType) {
    let runtime = TestCanisterRuntime::new().add_times([time; 4]);
    mutate_state(|state| process_event(state, payload, &runtime));
}

fn setup() {
    reset_state();
    reset_events();
    init_state();
    init_balance_to(2_000_000);
    record_at(
        100,
        EventType::Upgrade(UpgradeArgs {
            add_spl_tokens: Some(
                [mint_a(), mint_b()]
                    .into_iter()
                    .map(|mint| SplTokenRegistration {
                        mint: mint.to_string(),
                        token_program: spl_token_interface::id().to_string(),
                        decimals: 6,
                        ledger_id: ledger_of(mint),
                        index_id: None,
                        symbol: format!("ckTest{}", mint.to_bytes()[0]),
                        minimum_deposit_amount: 1,
                        deposit_fee: TOKEN_FEE,
                        deposits_paused: false,
                    })
                    .collect(),
            ),
            ..Default::default()
        }),
    );
    record_at(
        100,
        EventType::RequestedOperatingRefill {
            refill_id: 0,
            amount: 1_000_000,
            created_at_time: 100,
        },
    );
    record_at(
        100,
        EventType::CompletedOperatingRefill {
            refill_id: 0,
            burn_block_index: 42,
        },
    );
}

fn queued_event(deposit_id: u64, deposit: &QueuedSplDeposit) -> EventType {
    EventType::QueuedSplDeposit {
        deposit_id,
        account: deposit.account,
        mint: deposit.mint,
        address: deposit.address,
        balance: deposit.balance,
        fee: deposit.fee,
    }
}

fn deposit_of(owner: usize, mint: Address, balance: u64) -> QueuedSplDeposit {
    let account = account(owner);
    let owner = account_address(&schnorr_master_key(), &account);
    QueuedSplDeposit {
        account,
        mint,
        address: associated_token_address(&owner, &mint),
        balance,
        fee: TOKEN_FEE,
    }
}

/// Takes one deposit through queue, sweep, success and credit. Its net amount is then owed.
fn credit(owner: usize, mint: Address, credited_at: u64) -> u64 {
    let deposit_id = read_state(|state| state.spl_deposits().next_id());
    let deposit = deposit_of(owner, mint, 1_000 + deposit_id);
    record_at(100, queued_event(deposit_id, &deposit));
    let sweep = read_state(|state| {
        SplSweep::plan(
            deposit_id,
            deposit.clone(),
            state.supported_spl_token(&mint).unwrap(),
            &schnorr_master_key(),
        )
        .unwrap()
    });
    let signature = sweep_signature(deposit_id);
    record_at(
        100,
        EventType::SubmittedTransaction {
            signature,
            message: sweep.sweep_message(solana_hash::Hash::default()).into(),
            signers: sweep.signers().to_vec(),
            purpose: TransactionPurpose::SweepSplDeposit {
                deposit_id,
                block_height: DEFAULT_BLOCK_HEIGHT,
            },
        },
    );
    record_at(100, EventType::SucceededTransaction { signature });
    record_at(
        credited_at,
        EventType::CreditedSplSweep {
            signature,
            lamports_spent: 10_000,
            deposit_id,
            amount_to_mint: deposit.amount_to_mint(),
        },
    );
    deposit_id
}

fn mint_runtime<I: IntoIterator<Item = MintResult>>(results: I) -> TestCanisterRuntime {
    let mut runtime = TestCanisterRuntime::new().with_increasing_time();
    for result in results {
        runtime = runtime.add_stub_response(result);
    }
    runtime
}

fn expected_transfer_arg(deposit_id: u64, owner: usize, created_at_time: u64) -> TransferArg {
    TransferArg {
        from_subaccount: None,
        to: account(owner),
        fee: None,
        created_at_time: Some(created_at_time),
        memo: Some(Memo::from(MintMemo::sweep(sweep_signature(deposit_id), deposit_id)).into()),
        amount: NumTokens::from(1_000 + deposit_id - TOKEN_FEE),
    }
}

fn transfers_sent_by(runtime: &TestCanisterRuntime) -> Vec<(Principal, TransferArg)> {
    runtime
        .sent_update_calls()
        .iter()
        .map(|call| {
            assert_eq!(call.method, "icrc1_transfer");
            (call.canister_id, call.single_arg())
        })
        .collect()
}

fn recorded() -> Vec<Event> {
    with_event_iter(|events| events.collect())
}

/// Replays the recorded log through its stored bytes and its public Candid form.
fn assert_replay() {
    for event in recorded() {
        let decoded = Event::from_bytes(std::borrow::Cow::Owned(event.to_bytes().into_owned()));
        let wire = candid::encode_one(CandidEvent::from(event)).unwrap();
        let public: CandidEvent = candid::decode_one(&wire).unwrap();
        assert_eq!(Event::try_from(public).unwrap(), decoded);
    }
    let replayed = replayed_recorded_log();
    read_state(|state| assert_eq!(state, &replayed));
}

fn assert_no_sol_mint_metrics() {
    for reason in FailedMintReason::ALL {
        assert_eq!(failed_mint_attempt_count(reason), 0, "{reason:?}");
    }
}

fn assert_rejected(payload: EventType, reason: &str) {
    let mut state: State = read_state(Clone::clone);
    let before = state.clone();
    let count = recorded().len();
    let runtime = TestCanisterRuntime::new().add_times([100; 4]);
    let error = catch_unwind(AssertUnwindSafe(|| {
        process_event(&mut state, payload, &runtime)
    }))
    .expect_err("an invalid mint event must be rejected");
    let message = error
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| error.downcast_ref::<&str>().copied())
        .unwrap();
    assert!(message.contains(reason), "expected {reason}, got {message}");
    assert_eq!(state, before);
    assert_eq!(recorded().len(), count);
}

#[tokio::test]
async fn should_return_early_without_pending_mints_or_when_already_active() {
    setup();
    let idle = TestCanisterRuntime::new();
    process_pending_spl_mints(idle.clone()).await;
    assert!(idle.sent_update_calls().is_empty());
    assert_eq!(idle.set_timer_call_count(), 0);

    credit(1, mint_a(), CREDITED_AT_TIME);
    let before = read_state(Clone::clone);
    mutate_state(|state| {
        state.active_tasks_mut().insert(TaskType::MintSpl);
    });
    let active = TestCanisterRuntime::new();
    process_pending_spl_mints(active.clone()).await;
    assert!(active.sent_update_calls().is_empty());
    mutate_state(|state| {
        state.active_tasks_mut().remove(&TaskType::MintSpl);
    });
    read_state(|state| assert_eq!(state, &before));
}

#[tokio::test]
async fn should_mint_the_net_amount_on_the_token_ledger_and_release_the_pair() {
    setup();
    let deposit_id = credit(1, mint_a(), CREDITED_AT_TIME);
    let (balance, operating) =
        read_state(|state| (state.balance(), state.operating_balance().balance()));
    let runtime = mint_runtime([Ok(BLOCK_INDEX.into())]);

    process_pending_spl_mints(runtime.clone()).await;

    assert_eq!(
        transfers_sent_by(&runtime),
        vec![(
            ledger_of(mint_a()),
            expected_transfer_arg(deposit_id, 1, CREDITED_AT_TIME)
        )]
    );
    EventsAssert::from_recorded().expect_contains_event_eq(EventType::MintedSweptSplDeposit {
        deposit_id,
        mint_block_index: BLOCK_INDEX.into(),
    });
    read_state(|state| {
        let deposits = state.spl_deposits();
        assert!(deposits.pending_mints().is_empty());
        assert!(deposits.quarantined().is_empty());
        let minted = &deposits.minted()[&deposit_id];
        assert_eq!(minted.minted_amount, 990);
        assert_eq!(minted.mint_block_index, BLOCK_INDEX.into());
        assert_eq!(minted.deposit.signature, sweep_signature(deposit_id));
        assert_eq!(minted.deposit.deposit, deposit_of(1, mint_a(), 1_000));
        // The token fee stays collected in the treasury. The mint moves no SOL.
        assert_eq!(
            deposits.collected_deposit_fees(&mint_a()),
            u128::from(TOKEN_FEE)
        );
        assert_eq!(
            state.collected_spl_deposit_fees(),
            [
                CollectedSplDepositFees {
                    mint: mint_a().into(),
                    amount: u128::from(TOKEN_FEE),
                },
                CollectedSplDepositFees {
                    mint: mint_b().into(),
                    amount: 0,
                },
            ]
        );
        assert_eq!(state.balance(), balance);
        assert_eq!(state.operating_balance().balance(), operating);
        assert_eq!(deposits.in_flight_id(&account(1), &mint_a()), None);
        assert_eq!(deposits.latest_id(&account(1), &mint_a()), Some(deposit_id));
        assert_eq!(deposits.oldest_in_flight_queued_at(), None);
    });
    assert_eq!(runtime.set_timer_call_count(), 0);
    assert_no_sol_mint_metrics();
    assert_replay();

    // The released pair accepts a new deposit.
    record_at(200, queued_event(1, &deposit_of(1, mint_a(), 5_000)));
    read_state(|state| {
        assert_eq!(
            state.spl_deposits().in_flight_id(&account(1), &mint_a()),
            Some(1)
        )
    });
    assert_replay();
    let idle = TestCanisterRuntime::new();
    process_pending_spl_mints(idle.clone()).await;
    assert!(idle.sent_update_calls().is_empty());
}

#[tokio::test]
async fn should_record_a_duplicate_reply_as_minted() {
    setup();
    let deposit_id = credit(1, mint_a(), CREDITED_AT_TIME);
    let runtime = mint_runtime([Err(TransferError::Duplicate {
        duplicate_of: BlockIndex::from(BLOCK_INDEX),
    })]);

    process_pending_spl_mints(runtime).await;

    EventsAssert::from_recorded().expect_contains_event_eq(EventType::MintedSweptSplDeposit {
        deposit_id,
        mint_block_index: BLOCK_INDEX.into(),
    });
    read_state(|state| {
        assert_eq!(
            state.spl_deposits().minted()[&deposit_id].minted_amount,
            990
        );
        assert_eq!(
            state.spl_deposits().in_flight_id(&account(1), &mint_a()),
            None
        );
    });
    assert_replay();
}

#[tokio::test]
async fn should_retry_a_transient_failure_with_exactly_the_same_arguments() {
    type AddFailure = fn(TestCanisterRuntime) -> TestCanisterRuntime;
    let transient_failures: [(&str, AddFailure); 4] = [
        ("the ledger is temporarily unavailable", |runtime| {
            runtime.add_stub_response::<MintResult>(Err(TransferError::TemporarilyUnavailable))
        }),
        ("the ledger returns a generic error", |runtime| {
            runtime.add_stub_response::<MintResult>(Err(TransferError::GenericError {
                error_code: Nat::from(42_u8),
                message: "out of luck".to_string(),
            }))
        }),
        ("the minter clock is ahead of the ledger", |runtime| {
            runtime.add_stub_response::<MintResult>(Err(TransferError::CreatedInFuture {
                ledger_time: 0,
            }))
        }),
        ("the call to the ledger fails", |runtime| {
            runtime.add_stub_error(IcError::CallPerformFailed)
        }),
    ];
    for (name, add_failure) in transient_failures {
        setup();
        let deposit_id = credit(1, mint_a(), CREDITED_AT_TIME);
        let before = read_state(Clone::clone);
        let events = recorded().len();
        let failing = add_failure(TestCanisterRuntime::new().with_increasing_time());

        process_pending_spl_mints(failing.clone()).await;

        read_state(|state| assert_eq!(state, &before, "{name}"));
        assert_eq!(recorded().len(), events, "{name}");
        assert_eq!(failing.set_timer_call_count(), 0, "{name}");

        let retrying = mint_runtime([Ok(BLOCK_INDEX.into())]);
        process_pending_spl_mints(retrying.clone()).await;

        let expected = vec![(
            ledger_of(mint_a()),
            expected_transfer_arg(deposit_id, 1, CREDITED_AT_TIME),
        )];
        assert_eq!(transfers_sent_by(&failing), expected, "{name}");
        assert_eq!(transfers_sent_by(&retrying), expected, "{name}");
        read_state(|state| {
            assert!(
                state.spl_deposits().minted().contains_key(&deposit_id),
                "{name}"
            )
        });
        assert_no_sol_mint_metrics();
        assert_replay();
    }
}

#[tokio::test]
async fn should_quarantine_a_mint_that_cannot_be_retried_safely_and_keep_the_pair_locked() {
    struct Case {
        name: &'static str,
        runtime: TestCanisterRuntime,
        expected_ledger_calls: usize,
        traps: bool,
    }
    let stale_now = CREDITED_AT_TIME + LEDGER_DEDUPLICATION_WINDOW.as_nanos() as u64 + 1;
    let cases = [
        Case {
            name: "the pending mint is older than the deduplication window of the ledger",
            runtime: TestCanisterRuntime::new().add_times([stale_now; 3]),
            expected_ledger_calls: 0,
            traps: false,
        },
        Case {
            name: "the ledger rejects the mint as too old",
            runtime: mint_runtime([Err(TransferError::TooOld)]),
            expected_ledger_calls: 1,
            traps: false,
        },
        Case {
            name: "the ledger rejects the fee of the mint",
            runtime: mint_runtime([Err(TransferError::BadFee {
                expected_fee: Nat::from(10_u8),
            })]),
            expected_ledger_calls: 1,
            traps: false,
        },
        Case {
            name: "the ledger takes the mint for a burn below the minimum",
            runtime: mint_runtime([Err(TransferError::BadBurn {
                min_burn_amount: Nat::from(10_u8),
            })]),
            expected_ledger_calls: 1,
            traps: false,
        },
        Case {
            name: "the ledger reports insufficient funds on the minting account",
            runtime: mint_runtime([Err(TransferError::InsufficientFunds {
                balance: Nat::from(0_u8),
            })]),
            expected_ledger_calls: 1,
            traps: false,
        },
        Case {
            name: "the callback traps after the ledger minted",
            runtime: mint_runtime([Ok(Nat::from(u128::MAX))]),
            expected_ledger_calls: 1,
            traps: true,
        },
    ];
    for Case {
        name,
        runtime,
        expected_ledger_calls,
        traps,
    } in cases
    {
        setup();
        let deposit_id = credit(1, mint_a(), CREDITED_AT_TIME);

        let outcome = AssertUnwindSafe(process_pending_spl_mints(runtime.clone()))
            .catch_unwind()
            .await;

        assert_eq!(outcome.is_err(), traps, "{name}");
        assert_eq!(
            transfers_sent_by(&runtime).len(),
            expected_ledger_calls,
            "{name}"
        );
        EventsAssert::from_recorded()
            .expect_contains_event_eq(EventType::QuarantinedPendingSplMint { deposit_id });
        read_state(|state| {
            let deposits = state.spl_deposits();
            assert!(deposits.pending_mints().is_empty(), "{name}");
            assert!(deposits.minted().is_empty(), "{name}");
            // The owed net amount and its ledger arguments stay available for manual resolution.
            let quarantined = &deposits.quarantined()[&deposit_id];
            assert_eq!(
                quarantined.cause,
                SplQuarantineCause::MintUnresolved {
                    amount_to_mint: 990,
                    created_at_time: CREDITED_AT_TIME,
                },
                "{name}"
            );
            assert_eq!(
                quarantined.sweep_signature(),
                sweep_signature(deposit_id),
                "{name}"
            );
            assert_eq!(
                deposits.in_flight_id(&account(1), &mint_a()),
                Some(deposit_id),
                "{name}"
            );
        });
        assert_eq!(runtime.set_timer_call_count(), 0, "{name}");
        assert_no_sol_mint_metrics();
        if !traps {
            assert_replay();
        }
        // The locked pair rejects a new deposit, and the mint is not sent again.
        assert_rejected(
            queued_event(1, &deposit_of(1, mint_a(), 5_000)),
            "already has one in flight",
        );
        let idle = TestCanisterRuntime::new();
        process_pending_spl_mints(idle.clone()).await;
        assert!(idle.sent_update_calls().is_empty(), "{name}");
    }
}

#[tokio::test]
async fn should_mint_each_deposit_on_the_ledger_of_its_token_even_when_paused() {
    setup();
    let first = credit(1, mint_a(), CREDITED_AT_TIME);
    let second = credit(1, mint_b(), CREDITED_AT_TIME + 1);
    let third = credit(2, mint_a(), CREDITED_AT_TIME + 2);
    // A pause stops new deposits only. An accepted deposit still gets its mint.
    record_at(
        2_000,
        EventType::Upgrade(UpgradeArgs {
            update_spl_tokens: Some(vec![SplTokenUpdate {
                mint: mint_b().to_string(),
                minimum_deposit_amount: None,
                deposit_fee: Some(500),
                deposits_paused: Some(true),
            }]),
            ..Default::default()
        }),
    );
    let runtime = mint_runtime([Ok(1_u8.into()), Ok(2_u8.into()), Ok(3_u8.into())]);

    process_pending_spl_mints(runtime.clone()).await;

    assert_eq!(
        transfers_sent_by(&runtime),
        vec![
            (
                ledger_of(mint_a()),
                expected_transfer_arg(first, 1, CREDITED_AT_TIME)
            ),
            (
                ledger_of(mint_b()),
                expected_transfer_arg(second, 1, CREDITED_AT_TIME + 1)
            ),
            (
                ledger_of(mint_a()),
                expected_transfer_arg(third, 2, CREDITED_AT_TIME + 2)
            ),
        ]
    );
    read_state(|state| {
        let minted = state.spl_deposits().minted();
        assert_eq!(minted.len(), 3);
        // The captured fee applies, not the later fee of the token.
        assert_eq!(minted[&second].minted_amount, 1_000 + second - TOKEN_FEE);
        assert_eq!(minted[&second].mint_block_index, 2.into());
    });
    assert_replay();
}

#[tokio::test]
async fn should_rotate_past_failing_mints_and_run_again_only_after_progress() {
    const COUNT: usize = MAX_PENDING_MINTS_PER_ROUND + 1;
    setup();
    let ids: Vec<u64> = (1..=COUNT)
        .map(|owner| credit(owner, mint_a(), CREDITED_AT_TIME))
        .collect();
    let first_minted_id = |runtime: &TestCanisterRuntime| {
        let (_, arg) = transfers_sent_by(runtime).remove(0);
        ids.iter()
            .copied()
            .find(|id| arg == expected_transfer_arg(*id, *id as usize + 1, CREDITED_AT_TIME))
            .unwrap()
    };

    // A round of failures makes no progress, so it waits for the periodic timer.
    let failing = mint_runtime(
        (0..MAX_PENDING_MINTS_PER_ROUND).map(|_| Err(TransferError::TemporarilyUnavailable)),
    );
    process_pending_spl_mints(failing.clone()).await;
    assert_eq!(
        transfers_sent_by(&failing).len(),
        MAX_PENDING_MINTS_PER_ROUND
    );
    assert_eq!(first_minted_id(&failing), ids[0]);
    assert_eq!(failing.set_timer_call_count(), 0);

    // The next round starts past the first ten, then wraps. It mints ten and one is left.
    let minting = mint_runtime((0..MAX_PENDING_MINTS_PER_ROUND).map(|i| Ok((i as u64).into())));
    process_pending_spl_mints(minting.clone()).await;
    assert_eq!(first_minted_id(&minting), ids[MAX_PENDING_MINTS_PER_ROUND]);
    assert_eq!(minting.set_timer_delays(), vec![std::time::Duration::ZERO]);
    read_state(|state| {
        assert_eq!(
            state.spl_deposits().minted().len(),
            MAX_PENDING_MINTS_PER_ROUND
        );
        assert_eq!(state.spl_deposits().pending_mints().len(), 1);
    });

    let last = mint_runtime([Ok(99_u8.into())]);
    process_pending_spl_mints(last.clone()).await;
    assert_eq!(last.set_timer_call_count(), 0);
    read_state(|state| assert!(state.spl_deposits().pending_mints().is_empty()));
    assert_no_sol_mint_metrics();
    assert_replay();
}

#[tokio::test]
async fn should_mint_sol_and_spl_with_separate_guards_and_cursors() {
    const COUNT: usize = MAX_PENDING_MINTS_PER_ROUND + 1;
    setup();
    for owner in 1..=COUNT {
        credit(owner, mint_a(), CREDITED_AT_TIME);
    }
    let sol_accounts: Vec<_> = (1..=COUNT)
        .map(|index| {
            DepositFlow::queue(account(100 + index), 25_000_000)
                .sweep(signature(0x900 + index))
                .succeed()
                .credit_at(CREDITED_AT_TIME)
                .single_pending_mint()
                .account
        })
        .collect();

    // A failing SPL round moves only the SPL cursor.
    let spl = mint_runtime(
        (0..MAX_PENDING_MINTS_PER_ROUND).map(|_| Err(TransferError::TemporarilyUnavailable)),
    );
    process_pending_spl_mints(spl.clone()).await;
    assert!(
        transfers_sent_by(&spl)
            .iter()
            .all(|(ledger, _)| *ledger == ledger_of(mint_a()))
    );

    // The ckSOL round still starts at its first pending mint, on the ckSOL ledger.
    // A running SPL mint task does not block it.
    mutate_state(|state| {
        state.active_tasks_mut().insert(TaskType::MintSpl);
    });
    let sol = mint_runtime(
        (0..MAX_PENDING_MINTS_PER_ROUND).map(|_| Err(TransferError::TemporarilyUnavailable)),
    );
    process_pending_mints(sol.clone()).await;
    let sol_transfers = transfers_sent_by(&sol);
    assert_eq!(sol_transfers.len(), MAX_PENDING_MINTS_PER_ROUND);
    assert_eq!(sol_transfers[0].1.to, sol_accounts[0]);
    let cksol_ledger = read_state(|state| state.ledger_canister_id());
    assert!(
        sol_transfers
            .iter()
            .all(|(ledger, _)| *ledger == cksol_ledger)
    );
    mutate_state(|state| {
        state.active_tasks_mut().remove(&TaskType::MintSpl);
    });

    // A running ckSOL mint task does not block the SPL mints, which continue past the first ten.
    mutate_state(|state| {
        state.active_tasks_mut().insert(TaskType::Mint);
    });
    let spl_again = mint_runtime(std::iter::once(Ok(7_u8.into())).chain(
        (1..MAX_PENDING_MINTS_PER_ROUND).map(|_| Err(TransferError::TemporarilyUnavailable)),
    ));
    process_pending_spl_mints(spl_again.clone()).await;
    let (_, arg) = transfers_sent_by(&spl_again).remove(0);
    assert_eq!(arg.to, account(COUNT));
    // Only the SOL failures reach the ckSOL metric.
    assert_eq!(
        failed_mint_attempt_count(FailedMintReason::LedgerError),
        MAX_PENDING_MINTS_PER_ROUND as u64
    );
}

#[tokio::test]
async fn should_report_each_status_and_reject_a_quarantined_pair() {
    setup();
    assert_eq!(deposit_spl_status(0), DepositSplStatus::NotFound);

    // Queued, swept, finalized and minted.
    let minted = credit(1, mint_a(), CREDITED_AT_TIME);
    let finalized = DepositSplStatus::Finalized {
        signature: sweep_signature(minted).into(),
    };
    assert_eq!(deposit_spl_status(minted), finalized);
    record_at(
        2_000,
        EventType::MintedSweptSplDeposit {
            deposit_id: minted,
            mint_block_index: BLOCK_INDEX.into(),
        },
    );
    assert_eq!(
        deposit_spl_status(minted),
        DepositSplStatus::Minted {
            block_index: BLOCK_INDEX,
            minted_amount: 990,
        }
    );

    // A mint that cannot be retried safely keeps its pair rejected.
    let quarantined = credit(2, mint_a(), CREDITED_AT_TIME);
    record_at(
        2_000,
        EventType::QuarantinedPendingSplMint {
            deposit_id: quarantined,
        },
    );
    assert_eq!(
        deposit_spl_status(quarantined),
        DepositSplStatus::Quarantined {
            signature: sweep_signature(quarantined).into(),
        }
    );
    let runtime = TestCanisterRuntime::new();
    assert_eq!(
        deposit_spl(&runtime, account(2), mint_a().into()).await,
        Err(DepositSplError::Quarantined {
            deposit_id: quarantined
        })
    );
    assert!(runtime.sent_update_calls().is_empty());
    // The other mint of the same account is not affected.
    assert_eq!(
        read_state(|state| state.spl_deposits().in_flight(&account(2), &mint_b())),
        None
    );

    // Queued, swept, observed as failed, then dropped at its validated cost.
    let dropped = read_state(|state| state.spl_deposits().next_id());
    let deposit = deposit_of(3, mint_a(), 1_000);
    record_at(100, queued_event(dropped, &deposit));
    assert_eq!(
        deposit_spl_status(dropped),
        DepositSplStatus::Queued {
            balance: 1_000,
            fee: TOKEN_FEE,
        }
    );
    let sweep = read_state(|state| {
        SplSweep::plan(
            dropped,
            deposit,
            state.supported_spl_token(&mint_a()).unwrap(),
            &schnorr_master_key(),
        )
        .unwrap()
    });
    let signature = sweep_signature(dropped);
    record_at(
        100,
        EventType::SubmittedTransaction {
            signature,
            message: sweep.sweep_message(solana_hash::Hash::default()).into(),
            signers: sweep.signers().to_vec(),
            purpose: TransactionPurpose::SweepSplDeposit {
                deposit_id: dropped,
                block_height: DEFAULT_BLOCK_HEIGHT,
            },
        },
    );
    let swept = DepositSplStatus::Swept {
        signature: signature.into(),
    };
    assert_eq!(deposit_spl_status(dropped), swept);
    // An observed failure is not settled yet, so the deposit is still swept.
    record_at(100, EventType::FailedTransaction { signature });
    assert_eq!(deposit_spl_status(dropped), swept);
    record_at(
        100,
        EventType::FailedSplSweep {
            signature,
            lamports_spent: 5_000,
        },
    );
    assert_eq!(
        deposit_spl_status(dropped),
        DepositSplStatus::Dropped {
            signature: signature.into(),
        }
    );
    assert_replay();
}

#[test]
fn should_reject_mint_events_without_a_pending_mint_and_preserve_their_tags() {
    setup();
    let minted = |deposit_id| EventType::MintedSweptSplDeposit {
        deposit_id,
        mint_block_index: BLOCK_INDEX.into(),
    };
    let quarantined = |deposit_id| EventType::QuarantinedPendingSplMint { deposit_id };
    for (event, tag, fields) in [
        (minted(42), 19, vec![42, BLOCK_INDEX]),
        (quarantined(42), 20, vec![42]),
    ] {
        let encoded = minicbor::to_vec(&event).unwrap();
        let mut decoder = minicbor::Decoder::new(&encoded);
        assert_eq!(decoder.array().unwrap(), Some(2));
        assert_eq!(decoder.u32().unwrap(), tag);
        assert_eq!(decoder.array().unwrap(), Some(fields.len() as u64));
        for field in fields {
            assert_eq!(decoder.u64().unwrap(), field);
        }
        assert_eq!(decoder.position(), encoded.len());
        assert_eq!(minicbor::decode::<EventType>(&encoded).unwrap(), event);
    }

    // A deposit that is not credited has no pending mint.
    record_at(100, queued_event(0, &deposit_of(1, mint_a(), 1_000)));
    assert_rejected(minted(0), "has no pending mint");
    assert_rejected(quarantined(0), "has no pending mint");
    reset_state();
    reset_events();
    setup();

    let first = credit(1, mint_a(), CREDITED_AT_TIME);
    let second = credit(2, mint_a(), CREDITED_AT_TIME);
    record_at(2_000, minted(first));
    assert_rejected(minted(first), "has no pending mint");
    assert_rejected(quarantined(first), "has no pending mint");
    record_at(2_000, quarantined(second));
    assert_rejected(quarantined(second), "has no pending mint");
    assert_rejected(minted(second), "has no pending mint");
    assert_replay();
}
