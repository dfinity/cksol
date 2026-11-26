use super::*;
use crate::{
    constants::MAX_CONCURRENT_RPC_CALLS,
    monitor::{FINALIZE_TRANSACTIONS_RETRY_DELAY, finalize_transactions},
    state::{
        SplQuarantineCause, State,
        audit::{process_event, replay_events},
        event::{Event, EventType, TransactionPurpose},
        mutate_state, read_state, reset_state,
    },
    storage::{FailedCreditReason, failed_credit_attempt_count, reset_events, with_event_iter},
    test_fixtures::{
        DEFAULT_BLOCK_HEIGHT, EventsAssert, account, confirmed_block_at_height, devnet_sweep,
        events, finalized_status, init_args_with_nonce_account, init_state,
        runtime::TestCanisterRuntime, spl_sweep as fixture,
    },
};
use cksol_types_internal::{UpgradeArgs, event::Event as CandidEvent};
use ic_stable_structures::Storable;
use sol_rpc_types::{
    CommitmentLevel, GetBlockCommitmentLevel, GetBlockParams, GetSignatureStatusesParams,
    GetSlotParams, GetTransactionParams, MultiRpcResult, RpcConfig, RpcSources,
};
use solana_signature::Signature;
use solana_transaction::TransactionError;
use solana_transaction_status_client_types::{
    EncodedConfirmedTransactionWithStatusMeta, TransactionConfirmationStatus,
};
use std::panic::{AssertUnwindSafe, catch_unwind};

fn clock() -> TestCanisterRuntime {
    TestCanisterRuntime::new().add_times([100; 100])
}
fn record(payload: EventType) {
    mutate_state(|state| process_event(state, payload, &clock()));
}
fn signature(sweep: &crate::state::SplSweep) -> Signature {
    fixture::transaction(sweep).signatures[0]
}
fn setup(count: u64) -> Vec<crate::state::SplSweep> {
    reset_state();
    reset_events();
    init_state();
    fixture::init_balance(2_000_000);
    record(EventType::Upgrade(UpgradeArgs {
        add_spl_tokens: Some(vec![fixture::registration()]),
        ..Default::default()
    }));
    record(EventType::RequestedOperatingRefill {
        refill_id: 0,
        amount: 1_000_000,
        created_at_time: 100,
    });
    record(EventType::CompletedOperatingRefill {
        refill_id: 0,
        burn_block_index: 42,
    });
    let mut sweeps = Vec::new();
    for id in 0..count {
        let sweep = fixture::plan(id);
        let deposit = sweep.deposit();
        record(EventType::QueuedSplDeposit {
            deposit_id: id,
            account: deposit.account,
            mint: deposit.mint,
            address: deposit.address,
            balance: deposit.balance,
            fee: deposit.fee,
        });
        record(EventType::SubmittedTransaction {
            signature: signature(&sweep),
            message: sweep
                .sweep_message(solana_hash::Hash::new_from_array([0x42; 32]))
                .into(),
            signers: sweep.signers().to_vec(),
            purpose: TransactionPurpose::SweepSplDeposit {
                deposit_id: id,
                block_height: DEFAULT_BLOCK_HEIGHT,
            },
        });
        sweeps.push(sweep);
    }
    sweeps.sort_by_key(signature);
    sweeps
}
fn observation_runtime(
    age: u64,
    statuses: Vec<Option<sol_rpc_types::TransactionStatus>>,
) -> TestCanisterRuntime {
    TestCanisterRuntime::new()
        .add_times([1_000; 100])
        .add_stub_response(MultiRpcResult::Consistent(Ok(300_000_000u64)))
        .add_stub_response(MultiRpcResult::Consistent(Ok(confirmed_block_at_height(
            (DEFAULT_BLOCK_HEIGHT.get() + age).into(),
        ))))
        .add_stub_response(MultiRpcResult::Consistent(Ok(statuses)))
}
fn outcome(
    runtime: TestCanisterRuntime,
    value: Option<EncodedConfirmedTransactionWithStatusMeta>,
) -> TestCanisterRuntime {
    let value: Option<sol_rpc_types::EncodedConfirmedTransactionWithStatusMeta> =
        value.map(|value| value.try_into().unwrap());
    runtime.add_stub_response(MultiRpcResult::Consistent(Ok(value)))
}
fn recorded() -> Vec<Event> {
    with_event_iter(|events| events.collect())
}
fn replayed() -> State {
    let key = devnet_sweep::master_key();
    let prefix = [
        EventType::Init(init_args_with_nonce_account()),
        EventType::MinterPublicKeyFetched {
            public_key: key.public_key,
            chain_code: key.chain_code,
        },
    ]
    .into_iter()
    .map(|payload| Event {
        timestamp: 0,
        payload,
    });
    let events = recorded().into_iter().map(|event| {
        let bytes = event.to_bytes().into_owned();
        let decoded = Event::from_bytes(std::borrow::Cow::Owned(bytes));
        let public = CandidEvent::from(event);
        let wire = candid::encode_one(public).unwrap();
        let public: CandidEvent = candid::decode_one(&wire).unwrap();
        let converted = Event::try_from(public).unwrap();
        assert_eq!(decoded, converted);
        decoded
    });
    replay_events(prefix.chain(events))
}
fn assert_replay() {
    let replayed = replayed();
    read_state(|state| assert_eq!(state, &replayed));
}
/// The state after an upgrade: rebuilt from the event log, with the volatile cursors reset.
fn upgrade() {
    let replayed = replayed();
    mutate_state(|state| *state = replayed);
    crate::deposit::sweep::reset_cursors();
}
fn failed_status() -> sol_rpc_types::TransactionStatus {
    let mut status = finalized_status();
    status.status = Err(TransactionError::AccountNotFound.into());
    status.err = Some(TransactionError::AccountNotFound.into());
    status
}
fn failed_meta(
    sweep: &crate::state::SplSweep,
    fee: u64,
) -> solana_transaction_status_client_types::UiTransactionStatusMeta {
    use solana_transaction_status_client_types::option_serializer::OptionSerializer;
    let mut meta = fixture::meta(sweep);
    meta.err = Some(TransactionError::AccountNotFound.into());
    meta.status = Err(TransactionError::AccountNotFound.into());
    meta.fee = fee;
    meta.post_balances[0] = meta.pre_balances[0] - fee;
    // A failed transfer may have no token balances, for example with no treasury account.
    meta.pre_token_balances = OptionSerializer::None;
    meta.post_token_balances = OptionSerializer::None;
    meta
}
fn fetched_signatures(runtime: &TestCanisterRuntime) -> Vec<sol_rpc_types::Signature> {
    runtime
        .sent_update_calls()
        .iter()
        .filter(|call| call.method == "getTransaction")
        .map(|call| {
            let (_, _, params): (RpcSources, Option<RpcConfig>, GetTransactionParams) = call.args();
            params.signature
        })
        .collect()
}
fn methods(runtime: &TestCanisterRuntime) -> Vec<String> {
    runtime
        .sent_update_calls()
        .iter()
        .map(|call| call.method.clone())
        .collect()
}
fn assert_no_resubmission(runtime: &TestCanisterRuntime) {
    assert!(runtime.sent_transactions().is_empty());
    assert_eq!(runtime.set_timer_call_count(), 0);
}

#[tokio::test]
async fn should_credit_captured_net_and_fee_only_after_validated_success() {
    let sweeps = setup(1);
    let sweep = &sweeps[0];
    let sig = signature(sweep);
    let mut meta = fixture::meta(sweep);
    meta.fee = 5_000;
    meta.post_balances[0] = meta.pre_balances[0] - 5_000;
    // Current acceptance terms must not change the captured deposit or interrupt it.
    record(EventType::Upgrade(UpgradeArgs {
        update_spl_tokens: Some(vec![cksol_types_internal::SplTokenUpdate {
            mint: fixture::mint().to_string(),
            minimum_deposit_amount: Some(20_000),
            deposit_fee: Some(200),
            deposits_paused: Some(true),
        }]),
        ..Default::default()
    }));
    let runtime = outcome(
        observation_runtime(151, vec![Some(finalized_status())]),
        Some(fixture::outcome(sweep, Some(meta))),
    );
    finalize_transactions(runtime.clone()).await;
    read_state(|state| {
        assert!(state.spl_deposits().swept().is_empty());
        let pending = &state.spl_deposits().pending_mints()[&0];
        assert_eq!(pending.created_at_time, 1_000);
        assert_eq!(pending.amount_to_mint, 990);
        assert_eq!(pending.deposit.deposit.fee, 10);
        assert_eq!(pending.sweep_signature(), sig);
        assert_eq!(
            state
                .spl_deposits()
                .collected_deposit_fees(&fixture::mint()),
            10
        );
        assert_eq!(state.balance(), 1_995_000);
        assert_eq!(state.operating_balance().balance(), 995_000);
        assert_eq!(
            state
                .spl_deposits()
                .in_flight_id(&account(1), &fixture::mint()),
            Some(0)
        );
        assert_eq!(state.spl_deposits().oldest_in_flight_queued_at(), Some(100));
    });
    EventsAssert::from_recorded()
        .expect_contains_event_eq(EventType::SucceededTransaction { signature: sig })
        .expect_contains_event_eq(EventType::CreditedSplSweep {
            signature: sig,
            lamports_spent: 5_000,
            deposit_id: 0,
            amount_to_mint: 990,
        });
    let calls = runtime.sent_update_calls();
    let (_, _, slot): (RpcSources, Option<RpcConfig>, Option<GetSlotParams>) = calls[0].args();
    assert_eq!(slot.unwrap().commitment, Some(CommitmentLevel::Finalized));
    let (_, _, block): (RpcSources, Option<RpcConfig>, GetBlockParams) = calls[1].args();
    assert_eq!(block.commitment, Some(GetBlockCommitmentLevel::Finalized));
    let (_, _, statuses): (RpcSources, Option<RpcConfig>, GetSignatureStatusesParams) =
        calls[2].args();
    assert_eq!(statuses.search_transaction_history, Some(true));
    let (_, _, transaction): (RpcSources, Option<RpcConfig>, GetTransactionParams) =
        calls[3].args();
    assert_eq!(transaction.commitment, Some(CommitmentLevel::Finalized));
    assert_no_resubmission(&runtime);
    assert_replay();
    let before = read_state(Clone::clone);
    finalize_transactions(clock()).await;
    read_state(|state| assert_eq!(state, &before));
}

#[tokio::test]
async fn should_release_only_the_unspent_reservation_on_validated_failure() {
    for fee in [0, 4_000, 10_000] {
        let sweeps = setup(1);
        let sweep = &sweeps[0];
        let mut status = finalized_status();
        status.err = Some(TransactionError::AccountNotFound.into());
        let mut meta = fixture::meta(sweep);
        let error = solana_transaction_status_client_types::UiTransactionError::from(
            TransactionError::AccountNotFound,
        );
        meta.err = Some(error.clone());
        meta.status = Err(error);
        meta.fee = fee;
        meta.post_balances[0] = meta.pre_balances[0] - fee;
        meta.pre_token_balances =
            solana_transaction_status_client_types::option_serializer::OptionSerializer::None;
        meta.post_token_balances =
            solana_transaction_status_client_types::option_serializer::OptionSerializer::None;
        let runtime = outcome(
            observation_runtime(151, vec![Some(status)]),
            Some(fixture::outcome(sweep, Some(meta))),
        );
        finalize_transactions(runtime.clone()).await;
        read_state(|state| {
            assert!(state.spl_deposits().swept().is_empty());
            assert!(state.spl_deposits().pending_mints().is_empty());
            assert!(
                state
                    .spl_deposits()
                    .dropped()
                    .contains_key(&sweep.deposit_id())
            );
            assert_eq!(
                state
                    .spl_deposits()
                    .in_flight_id(&account(1), &fixture::mint()),
                None
            );
            assert_eq!(state.spl_deposits().oldest_in_flight_queued_at(), None);
            assert_eq!(
                state
                    .spl_deposits()
                    .collected_deposit_fees(&fixture::mint()),
                0
            );
            assert_eq!(state.balance(), 2_000_000 - fee);
            assert_eq!(state.operating_balance().balance(), 1_000_000 - fee);
        });
        EventsAssert::from_recorded()
            .expect_contains_event_eq(EventType::FailedTransaction {
                signature: signature(sweep),
            })
            .expect_contains_event_eq(EventType::FailedSplSweep {
                signature: signature(sweep),
                lamports_spent: fee,
            });
        assert_no_resubmission(&runtime);
        assert_replay();
    }
}

#[tokio::test]
async fn should_expire_only_a_missing_signature_beyond_the_finalized_block_window() {
    for age in [150, 151] {
        let sweeps = setup(1);
        let sig = signature(&sweeps[0]);
        let runtime = observation_runtime(age, vec![None]);
        finalize_transactions(runtime.clone()).await;
        read_state(|state| {
            if age == 150 {
                assert!(state.spl_deposits().swept().get(&sig).is_some());
                assert_eq!(state.balance(), 1_990_000);
            } else {
                assert!(state.spl_deposits().swept().is_empty());
                assert!(state.spl_deposits().dropped().contains_key(&0));
                assert_eq!(state.balance(), 2_000_000);
                assert_eq!(state.operating_balance().balance(), 1_000_000);
                assert_eq!(
                    state
                        .spl_deposits()
                        .in_flight_id(&account(1), &fixture::mint()),
                    None
                );
                assert_eq!(
                    state
                        .spl_deposits()
                        .latest_id(&account(1), &fixture::mint()),
                    Some(0)
                );
            }
        });
        assert_no_resubmission(&runtime);
        assert_replay();
        if age == 151 {
            let deposit = sweeps[0].deposit();
            record(EventType::QueuedSplDeposit {
                deposit_id: 1,
                account: deposit.account,
                mint: deposit.mint,
                address: deposit.address,
                balance: deposit.balance,
                fee: deposit.fee,
            });
            read_state(|state| {
                assert_eq!(
                    state
                        .spl_deposits()
                        .in_flight_id(&account(1), &fixture::mint()),
                    Some(1)
                )
            });
            assert_replay();
        }
    }
}

#[tokio::test]
async fn should_retain_reservations_for_processed_confirmed_and_unknown_results() {
    for confirmation in [
        TransactionConfirmationStatus::Processed,
        TransactionConfirmationStatus::Confirmed,
    ] {
        setup(1);
        let before = read_state(Clone::clone);
        let mut status = finalized_status();
        status.confirmation_status = Some(confirmation.into());
        let runtime = observation_runtime(500, vec![Some(status)]);
        finalize_transactions(runtime.clone()).await;
        read_state(|state| assert_eq!(state, &before));
        assert_no_resubmission(&runtime);
    }
    for missing in [true, false] {
        let sweeps = setup(1);
        let sweep = &sweeps[0];
        let response = if missing {
            None
        } else {
            Some(fixture::outcome(sweep, None))
        };
        let runtime = outcome(
            observation_runtime(500, vec![Some(finalized_status())]),
            response,
        );
        finalize_transactions(runtime.clone()).await;
        read_state(|state| {
            assert!(state.succeeded_transactions().contains(&signature(sweep)));
            assert!(state.spl_deposits().swept().is_empty());
            assert!(
                state
                    .spl_deposits()
                    .finalized()
                    .get(&signature(sweep))
                    .is_some()
            );
            assert!(state.spl_deposits().pending_mints().is_empty());
            assert_eq!(state.balance(), 1_990_000);
            assert_eq!(state.operating_balance().balance(), 990_000);
        });
        assert_no_resubmission(&runtime);
        assert_replay();
        // An observed success leaves the status check, so a later missing status cannot expire it.
        let retry = outcome(
            clock(),
            Some(fixture::outcome(sweep, Some(fixture::meta(sweep)))),
        );
        finalize_transactions(retry.clone()).await;
        assert_eq!(methods(&retry), vec!["getTransaction"]);
        read_state(|state| {
            assert!(state.spl_deposits().finalized().is_empty());
            assert_eq!(state.spl_deposits().pending_mints().len(), 1);
        });
        assert_no_resubmission(&retry);
        assert_replay();
    }
}

#[tokio::test]
async fn should_quarantine_only_the_mismatched_pair_and_keep_its_reservation() {
    let sweeps = setup(2);
    let mut runtime = observation_runtime(151, vec![Some(finalized_status()); 2]);
    for (index, sweep) in sweeps.iter().enumerate() {
        let mut meta = fixture::meta(sweep);
        if index == 0 {
            meta.post_balances[0] -= 1;
        }
        runtime = outcome(runtime, Some(fixture::outcome(sweep, Some(meta))));
    }
    finalize_transactions(runtime.clone()).await;
    read_state(|state| {
        assert!(state.spl_deposits().swept().is_empty());
        assert!(state.spl_deposits().finalized().is_empty());
        let quarantined = &state.spl_deposits().quarantined()[&sweeps[0].deposit_id()];
        assert_eq!(quarantined.sweep_signature(), signature(&sweeps[0]));
        assert_eq!(
            quarantined.cause,
            SplQuarantineCause::SweepUnreadable {
                reserved_lamports: 10_000
            }
        );
        assert_eq!(state.spl_deposits().pending_mints().len(), 1);
        assert_eq!(
            state
                .spl_deposits()
                .collected_deposit_fees(&fixture::mint()),
            10
        );
        assert_eq!(state.balance(), 1_980_000);
        assert_eq!(state.operating_balance().balance(), 980_000);
        for sweep in &sweeps {
            assert_eq!(
                state
                    .spl_deposits()
                    .in_flight_id(&sweep.deposit().account, &fixture::mint()),
                Some(sweep.deposit_id())
            );
        }
    });
    assert_no_resubmission(&runtime);
    assert_replay();
    let idle = clock();
    finalize_transactions(idle.clone()).await;
    assert!(idle.sent_update_calls().is_empty());
}

#[tokio::test]
async fn should_keep_an_observed_failure_reserved_across_missing_statuses_and_upgrades() {
    let sweeps = setup(1);
    let sweep = &sweeps[0];
    let sig = signature(sweep);
    let assert_reserved = || {
        read_state(|state| {
            assert!(state.spl_deposits().swept().is_empty());
            assert_eq!(
                state
                    .spl_deposits()
                    .failed()
                    .get(&sig)
                    .unwrap()
                    .reserved_lamports,
                10_000
            );
            assert!(state.spl_deposits().dropped().is_empty());
            assert_eq!(state.balance(), 1_990_000);
            assert_eq!(state.operating_balance().balance(), 990_000);
            assert_eq!(
                state
                    .spl_deposits()
                    .in_flight_id(&account(1), &fixture::mint()),
                Some(0)
            );
        })
    };
    // 1. The failure is observed, but its transaction cannot be read.
    let observed = outcome(observation_runtime(500, vec![Some(failed_status())]), None);
    finalize_transactions(observed.clone()).await;
    EventsAssert::from_recorded()
        .expect_contains_event_eq(EventType::FailedTransaction { signature: sig });
    assert_reserved();
    assert_no_resubmission(&observed);
    assert_replay();

    // 2. An upgrade must not lose the observation.
    upgrade();
    assert_reserved();

    // 3. The sweep is long past the expiry window, and its transaction is still unreadable.
    // An observed failure is not status-checked again, so a missing status cannot expire it.
    let missing = outcome(clock(), None);
    finalize_transactions(missing.clone()).await;
    assert_eq!(methods(&missing), vec!["getTransaction"]);
    assert_reserved();
    assert_rejected(
        EventType::ExpiredTransaction { signature: sig },
        "already failed transaction",
    );
    assert_no_resubmission(&missing);

    // 4. Validated metadata settles the actual cost and releases only the remainder.
    let settled = outcome(
        clock(),
        Some(fixture::outcome(sweep, Some(failed_meta(sweep, 4_000)))),
    );
    finalize_transactions(settled.clone()).await;
    EventsAssert::from_recorded().expect_contains_event_eq(EventType::FailedSplSweep {
        signature: sig,
        lamports_spent: 4_000,
    });
    read_state(|state| {
        assert!(state.spl_deposits().failed().is_empty());
        assert!(state.spl_deposits().dropped().contains_key(&0));
        assert_eq!(state.balance(), 1_996_000);
        assert_eq!(state.operating_balance().balance(), 996_000);
        assert_eq!(
            state
                .spl_deposits()
                .in_flight_id(&account(1), &fixture::mint()),
            None
        );
    });
    assert_no_resubmission(&settled);
    assert_replay();
}

#[tokio::test]
async fn should_process_sol_and_spl_sweeps_from_one_status_check() {
    let sweeps = setup(1);
    let spl = &sweeps[0];
    let spl_sig = signature(spl);
    let sol_id = read_state(|state| state.deposits().next_id());
    events::queue(
        sol_id,
        devnet_sweep::fresh_deposit(account(0xA0), 1_000_000),
    );
    let sol_sig = crate::test_fixtures::signature(7);
    events::submit_sweep_to(sol_sig, vec![sol_id], devnet_sweep::minter_main_address());
    let not_found = failed_credit_attempt_count(FailedCreditReason::NotFound);

    // The SOL sweep failed and the SPL sweep succeeded. Statuses follow the signature order.
    let mut signatures = [sol_sig, spl_sig];
    signatures.sort();
    let statuses = signatures
        .iter()
        .map(|signature| {
            Some(if *signature == sol_sig {
                failed_status()
            } else {
                finalized_status()
            })
        })
        .collect();
    let runtime = outcome(
        observation_runtime(0, statuses),
        Some(fixture::outcome(spl, Some(fixture::meta(spl)))),
    );
    finalize_transactions(runtime.clone()).await;

    let calls = runtime.sent_update_calls();
    assert_eq!(
        methods(&runtime),
        vec![
            "getSlot",
            "getBlock",
            "getSignatureStatuses",
            "getTransaction"
        ]
    );
    let (_, _, params): (RpcSources, Option<RpcConfig>, GetSignatureStatusesParams) =
        calls[2].args();
    let queried: Vec<sol_rpc_types::Signature> = params.signatures.into();
    let expected: Vec<sol_rpc_types::Signature> = signatures
        .iter()
        .map(|signature| (*signature).into())
        .collect();
    assert_eq!(queried, expected);
    assert_eq!(fetched_signatures(&runtime), vec![spl_sig.into()]);
    read_state(|state| {
        // The SOL failure is terminal, as before. It is not an SPL failure awaiting its cost.
        assert!(state.failed_transactions().contains_key(&sol_sig));
        assert!(state.submitted_transactions().is_empty());
        assert!(state.deposits().swept().is_empty());
        assert!(state.spl_deposits().failed().is_empty());
        assert_eq!(
            state.spl_deposits().pending_mints()[&0].sweep_signature(),
            spl_sig
        );
        assert!(state.succeeded_transactions().contains(&spl_sig));
    });
    EventsAssert::from_recorded()
        .expect_contains_event_eq(EventType::FailedTransaction { signature: sol_sig });
    EventsAssert::from_recorded()
        .expect_contains_event_eq(EventType::SucceededTransaction { signature: spl_sig });
    assert_eq!(
        failed_credit_attempt_count(FailedCreditReason::NotFound),
        not_found
    );
    assert_no_resubmission(&runtime);
    assert_replay();
}

#[tokio::test]
async fn should_not_expire_on_failed_or_incomplete_status_reads_or_unknown_height() {
    for case in 0..3 {
        setup(1);
        let before = read_state(Clone::clone);
        let runtime = if case == 0 {
            clock()
                .add_recent_block(Err(sol_rpc_types::RpcError::ValidationError(
                    "no height".into(),
                )))
                .add_stub_response(MultiRpcResult::<
                    Vec<Option<sol_rpc_types::TransactionStatus>>,
                >::Consistent(Ok(vec![None])))
        } else {
            let runtime = clock()
                .add_stub_response(MultiRpcResult::Consistent(Ok(300_000_000u64)))
                .add_stub_response(MultiRpcResult::Consistent(Ok(confirmed_block_at_height(
                    (DEFAULT_BLOCK_HEIGHT.get() + 500).into(),
                ))));
            if case == 1 {
                runtime.add_stub_error(ic_canister_runtime::IcError::CallPerformFailed)
            } else {
                runtime.add_stub_response(MultiRpcResult::<
                    Vec<Option<sol_rpc_types::TransactionStatus>>,
                >::Consistent(Ok(vec![])))
            }
        };
        finalize_transactions(runtime.clone()).await;
        read_state(|state| assert_eq!(state, &before));
        assert_no_resubmission(&runtime);
        assert_replay();
    }
}

#[tokio::test]
async fn should_retain_a_reservation_when_the_transaction_response_cannot_be_trusted() {
    let sweeps = setup(1);
    let sweep = &sweeps[0];
    let mut fetched = fixture::outcome(sweep, Some(fixture::meta(sweep)));
    // Another valid signed transaction must not be attributed to the queried signature.
    fetched.transaction.transaction =
        fixture::outcome(&fixture::plan(1), Some(fixture::meta(&fixture::plan(1))))
            .transaction
            .transaction;
    let runtime = outcome(
        observation_runtime(500, vec![Some(finalized_status())]),
        Some(fetched),
    );
    finalize_transactions(runtime.clone()).await;
    read_state(|state| {
        assert!(
            state
                .spl_deposits()
                .finalized()
                .get(&signature(sweep))
                .is_some()
        );
        assert!(state.spl_deposits().pending_mints().is_empty());
        assert_eq!(state.operating_balance().balance(), 990_000);
    });
    assert_no_resubmission(&runtime);
    assert_replay();
}

#[tokio::test]
async fn should_quarantine_disagreement_between_finalized_status_and_transaction_metadata() {
    for expected_success in [true, false] {
        let sweeps = setup(1);
        let sweep = &sweeps[0];
        let mut status = finalized_status();
        let mut meta = fixture::meta(sweep);
        if expected_success {
            meta.err = Some(TransactionError::AccountNotFound.into());
            meta.status = Err(TransactionError::AccountNotFound.into());
        } else {
            status.err = Some(TransactionError::AccountNotFound.into());
        }
        let runtime = outcome(
            observation_runtime(500, vec![Some(status)]),
            Some(fixture::outcome(sweep, Some(meta))),
        );
        finalize_transactions(runtime.clone()).await;
        read_state(|state| {
            assert!(
                state
                    .spl_deposits()
                    .quarantined()
                    .contains_key(&sweep.deposit_id())
            );
            assert!(state.spl_deposits().pending_mints().is_empty());
            assert!(state.spl_deposits().dropped().is_empty());
            assert_eq!(state.operating_balance().balance(), 990_000);
            assert_eq!(state.balance(), 1_990_000);
        });
        assert_no_resubmission(&runtime);
        assert_replay();
    }
}

#[tokio::test]
async fn should_advance_past_unreadable_outcomes_in_bounded_rounds() {
    const COUNT: usize = MAX_CONCURRENT_RPC_CALLS + 1;
    for failed in [false, true] {
        let sweeps = setup(COUNT as u64);
        let not_found = failed_credit_attempt_count(FailedCreditReason::NotFound);
        let status = if failed {
            failed_status()
        } else {
            finalized_status()
        };
        for round in 0..2 {
            // One status check observes every sweep. Later rounds only read transactions.
            let mut runtime = if round == 0 {
                observation_runtime(500, vec![Some(status.clone()); COUNT])
            } else {
                clock()
            };
            for _ in 0..MAX_CONCURRENT_RPC_CALLS {
                runtime = outcome(runtime, None);
            }
            finalize_transactions(runtime.clone()).await;
            let status_checks = methods(&runtime)
                .iter()
                .filter(|method| *method == "getSignatureStatuses")
                .count();
            assert_eq!(status_checks, if round == 0 { 1 } else { 0 });
            // Every read is unreadable. The second round starts past the first ten.
            let fetched = fetched_signatures(&runtime);
            assert_eq!(fetched.len(), MAX_CONCURRENT_RPC_CALLS);
            let first = if round == 0 {
                0
            } else {
                MAX_CONCURRENT_RPC_CALLS
            };
            assert_eq!(fetched[0], signature(&sweeps[first]).into());
            assert_no_resubmission(&runtime);
        }
        read_state(|state| {
            let deposits = state.spl_deposits();
            let (observed, other) = if failed {
                (deposits.failed(), deposits.finalized())
            } else {
                (deposits.finalized(), deposits.failed())
            };
            assert!(deposits.swept().is_empty());
            assert_eq!(observed.len(), COUNT);
            assert!(other.is_empty());
            assert!(deposits.pending_mints().is_empty());
            assert!(deposits.dropped().is_empty());
            assert_eq!(
                state.operating_balance().balance(),
                1_000_000 - 10_000 * COUNT as u64
            );
        });
        // The failed-credit metrics count SOL sweeps only.
        assert_eq!(
            failed_credit_attempt_count(FailedCreditReason::NotFound),
            not_found
        );
        assert_replay();
    }
}

#[tokio::test]
async fn should_record_success_without_crediting_or_releasing_until_the_credit_step() {
    let sweeps = setup(1);
    let sweep = &sweeps[0];
    let sig = signature(sweep);
    // The status is readable, but the transaction is not: only the observation can happen.
    let observation = outcome(
        observation_runtime(151, vec![Some(finalized_status())]),
        None,
    );
    finalize_transactions(observation.clone()).await;
    assert_eq!(
        methods(&observation),
        vec![
            "getSlot",
            "getBlock",
            "getSignatureStatuses",
            "getTransaction"
        ]
    );
    EventsAssert::from_recorded()
        .expect_contains_event_eq(EventType::SucceededTransaction { signature: sig });
    read_state(|state| {
        assert!(state.succeeded_transactions().contains(&sig));
        assert!(state.spl_deposits().swept().is_empty());
        assert_eq!(
            state
                .spl_deposits()
                .finalized()
                .get(&sig)
                .unwrap()
                .reserved_lamports,
            10_000
        );
        assert!(state.spl_deposits().pending_mints().is_empty());
        assert_eq!(
            state
                .spl_deposits()
                .collected_deposit_fees(&fixture::mint()),
            0
        );
        assert_eq!(state.balance(), 1_990_000);
        assert_eq!(state.operating_balance().balance(), 990_000);
        assert_eq!(
            state
                .spl_deposits()
                .in_flight_id(&account(1), &fixture::mint()),
            Some(0)
        );
    });
    assert_replay();

    let mut meta = fixture::meta(sweep);
    meta.fee = 5_000;
    meta.post_balances[0] = meta.pre_balances[0] - 5_000;
    let credit = outcome(clock(), Some(fixture::outcome(sweep, Some(meta))));
    assert!(!credit_finalized_spl_sweeps(&credit).await);
    assert_eq!(methods(&credit), vec!["getTransaction"]);
    EventsAssert::from_recorded().expect_contains_event_eq(EventType::CreditedSplSweep {
        signature: sig,
        lamports_spent: 5_000,
        deposit_id: 0,
        amount_to_mint: 990,
    });
    read_state(|state| {
        assert!(state.spl_deposits().finalized().is_empty());
        let pending = &state.spl_deposits().pending_mints()[&0];
        assert_eq!(pending.amount_to_mint, 990);
        assert_eq!(pending.created_at_time, 100);
        assert_eq!(
            state
                .spl_deposits()
                .collected_deposit_fees(&fixture::mint()),
            10
        );
        assert_eq!(state.balance(), 1_995_000);
        assert_eq!(state.operating_balance().balance(), 995_000);
    });
    assert_replay();
}

#[tokio::test]
async fn should_run_again_soon_only_when_a_settlement_round_leaves_observed_sweeps() {
    for failed in [false, true] {
        let sweeps = setup(MAX_CONCURRENT_RPC_CALLS as u64 + 1);
        let fetched = |sweep: &crate::state::SplSweep| {
            let meta = if failed {
                failed_meta(sweep, 10_000)
            } else {
                fixture::meta(sweep)
            };
            Some(fixture::outcome(sweep, Some(meta)))
        };
        let observed = |state: &State| {
            let deposits = state.spl_deposits();
            let queue = if failed {
                deposits.failed()
            } else {
                deposits.finalized()
            };
            queue.signatures().copied().collect::<Vec<_>>()
        };
        let settled = |state: &State| {
            if failed {
                state.spl_deposits().dropped().len()
            } else {
                state.spl_deposits().pending_mints().len()
            }
        };
        for sweep in &sweeps {
            let signature = signature(sweep);
            record(if failed {
                EventType::FailedTransaction { signature }
            } else {
                EventType::SucceededTransaction { signature }
            });
        }
        let mut first = clock();
        for sweep in &sweeps[..MAX_CONCURRENT_RPC_CALLS] {
            first = outcome(first, fetched(sweep));
        }
        finalize_transactions(first.clone()).await;
        // Nothing is left to observe: the round reads the observed transactions only.
        assert_eq!(
            methods(&first),
            vec!["getTransaction"; MAX_CONCURRENT_RPC_CALLS]
        );
        assert_eq!(
            first.set_timer_delays(),
            vec![FINALIZE_TRANSACTIONS_RETRY_DELAY]
        );
        let last = &sweeps[MAX_CONCURRENT_RPC_CALLS];
        read_state(|state| {
            assert_eq!(settled(state), MAX_CONCURRENT_RPC_CALLS);
            assert_eq!(observed(state), vec![signature(last)]);
        });
        let second = outcome(clock(), fetched(last));
        finalize_transactions(second.clone()).await;
        assert_eq!(second.set_timer_call_count(), 0);
        read_state(|state| {
            assert!(observed(state).is_empty());
            assert_eq!(settled(state), MAX_CONCURRENT_RPC_CALLS + 1);
        });
        assert_replay();
    }
}

#[tokio::test]
async fn should_prevent_overlapping_finalization_rounds() {
    let sweeps = setup(1);
    let sweep = &sweeps[0];
    let runtime = outcome(
        observation_runtime(0, vec![Some(finalized_status())]),
        Some(fixture::outcome(sweep, Some(fixture::meta(sweep)))),
    );
    let concurrent = clock();
    tokio::join!(
        finalize_transactions(runtime.clone()),
        finalize_transactions(concurrent.clone())
    );
    assert!(concurrent.sent_update_calls().is_empty());
    read_state(|state| assert_eq!(state.spl_deposits().pending_mints().len(), 1));
    assert_replay();
}

fn assert_rejected(payload: EventType, reason: &str) {
    let mut state: State = read_state(Clone::clone);
    let before = state.clone();
    let count = recorded().len();
    let error = catch_unwind(AssertUnwindSafe(|| {
        process_event(&mut state, payload, &clock())
    }))
    .expect_err("invalid outcome event must be rejected");
    let message = error
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| error.downcast_ref::<&str>().copied())
        .unwrap();
    assert!(message.contains(reason), "expected {reason}, got {message}");
    assert_eq!(state, before);
    assert_eq!(recorded().len(), count);
}

#[test]
fn should_reject_invalid_credits_and_double_settlement_without_changing_state() {
    let sweeps = setup(1);
    let sig = signature(&sweeps[0]);
    let credit = |spent, deposit_id, amount_to_mint| EventType::CreditedSplSweep {
        signature: sig,
        lamports_spent: spent,
        deposit_id,
        amount_to_mint,
    };
    assert_rejected(credit(10_000, 0, 990), "is not finalized");
    // A failure cost cannot settle before the failure itself is recorded.
    assert_rejected(
        EventType::FailedSplSweep {
            signature: sig,
            lamports_spent: 10_000,
        },
        "failure was not recorded",
    );
    assert_rejected(
        EventType::QuarantinedSplSweep { signature: sig },
        "without an observed outcome",
    );
    record(EventType::SucceededTransaction { signature: sig });
    assert_rejected(
        EventType::SucceededTransaction { signature: sig },
        "unknown transaction",
    );
    assert_rejected(
        EventType::FailedTransaction { signature: sig },
        "already succeeded transaction",
    );
    assert_rejected(credit(10_000, 1, 990), "is not part of sweep");
    assert_rejected(
        credit(10_000, 0, 991),
        "different from its transferred balance minus the fee",
    );
    assert_rejected(credit(10_001, 0, 990), "above its reserved cost");
    assert_rejected(
        EventType::FailedSplSweep {
            signature: sig,
            lamports_spent: 10_000,
        },
        "failure was not recorded",
    );
    assert_rejected(
        EventType::ExpiredTransaction { signature: sig },
        "already succeeded transaction",
    );
    record(credit(10_000, 0, 990));
    assert_rejected(credit(10_000, 0, 990), "is not finalized");
    assert_replay();
}

#[test]
fn should_reject_invalid_events_after_an_observed_failure_without_changing_state() {
    let sweeps = setup(1);
    let sweep = &sweeps[0];
    let sig = signature(sweep);
    let settle = |spent| EventType::FailedSplSweep {
        signature: sig,
        lamports_spent: spent,
    };
    record(EventType::FailedTransaction { signature: sig });
    assert_rejected(
        EventType::FailedTransaction { signature: sig },
        "unknown transaction",
    );
    assert_rejected(
        EventType::SucceededTransaction { signature: sig },
        "already failed transaction",
    );
    assert_rejected(
        EventType::ExpiredTransaction { signature: sig },
        "already failed transaction",
    );
    assert_rejected(
        EventType::CreditedSplSweep {
            signature: sig,
            lamports_spent: 10_000,
            deposit_id: 0,
            amount_to_mint: 990,
        },
        "is not finalized",
    );
    assert_rejected(settle(10_001), "above its reserved cost");
    assert_rejected(
        EventType::SubmittedTransaction {
            signature: sig,
            message: sweep
                .sweep_message(solana_hash::Hash::new_from_array([0x42; 32]))
                .into(),
            signers: sweep.signers().to_vec(),
            purpose: TransactionPurpose::SweepSplDeposit {
                deposit_id: 0,
                block_height: DEFAULT_BLOCK_HEIGHT,
            },
        },
        "already failed transaction",
    );
    record(settle(10_000));
    assert_rejected(settle(10_000), "failure was not recorded");
    read_state(|state| {
        assert_eq!(state.balance(), 1_990_000);
        assert_eq!(state.operating_balance().balance(), 990_000);
    });
    assert_replay();
}

#[test]
fn should_record_dropped_deposits_by_id_and_preserve_new_event_tags() {
    let sweeps = setup(1);
    let sweep = &sweeps[0];
    let sig = signature(sweep);
    for (event, tag, fields) in [
        (
            EventType::CreditedSplSweep {
                signature: sig,
                lamports_spent: 10_000,
                deposit_id: 0,
                amount_to_mint: 990,
            },
            17,
            vec![10_000, 0, 990],
        ),
        (
            EventType::QuarantinedSplSweep { signature: sig },
            18,
            vec![],
        ),
        (
            EventType::FailedSplSweep {
                signature: sig,
                lamports_spent: 5_000,
            },
            35,
            vec![5_000],
        ),
    ] {
        let encoded = minicbor::to_vec(event).unwrap();
        let mut decoder = minicbor::Decoder::new(&encoded);
        assert_eq!(decoder.array().unwrap(), Some(2));
        assert_eq!(decoder.u32().unwrap(), tag);
        assert_eq!(decoder.array().unwrap(), Some(1 + fields.len() as u64));
        assert_eq!(decoder.bytes().unwrap(), sig.as_ref());
        for field in fields {
            assert_eq!(decoder.u64().unwrap(), field);
        }
        assert_eq!(decoder.position(), encoded.len());
    }
    record(EventType::ExpiredTransaction { signature: sig });
    let deposit = sweep.deposit();
    record(EventType::QueuedSplDeposit {
        deposit_id: 1,
        account: deposit.account,
        mint: deposit.mint,
        address: deposit.address,
        balance: deposit.balance,
        fee: deposit.fee,
    });
    read_state(|state| {
        let deposits = state.spl_deposits();
        assert_eq!(deposits.dropped()[&0].signature, sig);
        assert_eq!(
            deposits.in_flight_id(&deposit.account, &deposit.mint),
            Some(1)
        );
        assert!(!state.submitted_transactions().contains_key(&sig));
        assert!(!state.failed_transactions().contains_key(&sig));
    });
    assert_replay();
}
