use super::*;
use crate::{
    state::{State, audit::replay_events, event::Event, init_once_state, reset_state},
    storage::{operating_refill_error_count, reset_events, with_event_iter, with_unstable_metrics},
    test_fixtures::{
        init_args_with_nonce_account, init_state,
        runtime::{TEST_CANISTER_ID, TestCanisterRuntime},
    },
};
use candid::Nat;
use cksol_types_internal::{OperatingRefillConfig, UpgradeArgs};
use ic_canister_runtime::IcError;
use ic_stable_structures::Storable;
use std::borrow::Cow;

const TARGET: u64 = 1_000_000;

fn configure(config: OperatingRefillConfig) {
    record_event(
        &TestCanisterRuntime::new().with_increasing_time(),
        EventType::Upgrade(UpgradeArgs {
            operating_balance_refill: Some(config),
            ..Default::default()
        }),
    );
}

fn setup() {
    reset_state();
    reset_events();
    init_state();
    configure(OperatingRefillConfig {
        enabled: true,
        low_water_mark: 100_000,
        target: TARGET,
    });
}

fn runtime() -> TestCanisterRuntime {
    TestCanisterRuntime::new().with_increasing_time_from(1_000)
}

fn top_up_balance_read() -> Option<TopUpBalanceRead> {
    with_unstable_metrics(|m| m.operating_top_up_balance)
}

fn errors(reason: OperatingRefillErrorReason) -> u64 {
    operating_refill_error_count(reason)
}

fn metrics() -> String {
    let mut encoder = ic_metrics_encoder::MetricsEncoder::new(Vec::new(), 0);
    read_state(|s| crate::metrics::encode_operating_balance_metrics(&mut encoder, s)).unwrap();
    String::from_utf8(encoder.into_inner()).unwrap()
}

fn assert_metrics(expected: &[&str]) {
    let metrics = metrics();
    for line in expected {
        assert!(metrics.contains(line), "missing {line} in {metrics}");
    }
}

fn recorded_created_at_time() -> u64 {
    with_event_iter(|events| {
        events
            .filter_map(|event| match event.payload {
                EventType::RequestedOperatingRefill {
                    created_at_time, ..
                } => Some(created_at_time),
                _ => None,
            })
            .last()
    })
    .expect("no refill intent in the event log")
}

fn pending() -> PendingOperatingRefill {
    read_state(|s| s.operating_balance().pending().unwrap())
}

fn replay() -> State {
    let init = Event {
        timestamp: 0,
        payload: EventType::Init(init_args_with_nonce_account()),
    };
    let events: Vec<_> = with_event_iter(|events| {
        events
            .map(|e| Event::from_bytes(Cow::Owned(e.to_bytes().into_owned())))
            .collect()
    });
    replay_events(std::iter::once(init).chain(events))
}

#[tokio::test]
async fn should_do_nothing_when_refills_are_explicitly_disabled() {
    init_state();
    configure(OperatingRefillConfig {
        enabled: false,
        ..Default::default()
    });
    let runtime = TestCanisterRuntime::new();
    refill_operating_balance(runtime.clone()).await;
    assert!(runtime.sent_update_calls().is_empty());
    assert_eq!(read_state(|s| s.operating_balance().balance()), 0);
}

#[tokio::test]
async fn should_burn_up_to_the_target_or_the_available_top_up() {
    for (available, burned) in [
        (TARGET / 2, TARGET / 2),
        (TARGET, TARGET),
        (TARGET * 2, TARGET),
    ] {
        setup();
        let runtime = runtime()
            .add_stub_response(Nat::from(available))
            .add_stub_response(Ok::<Nat, TransferError>(42u8.into()));

        // Burn ckSOL
        refill_operating_balance(runtime.clone()).await;
        let burn = runtime.sent_update_calls()[1].single_arg::<TransferArg>();
        assert_eq!(burn.amount, Nat::from(burned));
        assert_eq!(read_state(|s| s.operating_balance().balance()), burned);
        assert_eq!(read_state(State::balance), 0);
        assert!(read_state(|s| s.operating_balance().pending()).is_none());
        let expected = replay();
        read_state(|s| assert_eq!(s, &expected));
    }
}

#[tokio::test]
async fn should_burn_with_the_arguments_of_the_recorded_intent() {
    setup();
    let runtime = runtime()
        .add_stub_response(Nat::from(TARGET))
        .add_stub_response(Ok::<Nat, TransferError>(42u8.into()));
    refill_operating_balance(runtime.clone()).await;
    let calls = runtime.sent_update_calls();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[1].method, "icrc1_transfer");
    assert_eq!(
        calls[1].single_arg::<TransferArg>(),
        TransferArg {
            from_subaccount: Some(OPERATING_TOP_UP_SUBACCOUNT),
            to: Account {
                owner: TEST_CANISTER_ID,
                subaccount: None,
            },
            fee: None,
            created_at_time: Some(recorded_created_at_time()),
            memo: Some(Memo::from(BurnMemo::OperatingRefill { refill_id: 0 }).into()),
            amount: Nat::from(TARGET),
        }
    );
}

#[tokio::test]
async fn should_read_the_top_up_account_and_not_burn_an_empty_balance() {
    setup();
    let runtime = runtime().add_stub_response(Nat::from(0u8));
    refill_operating_balance(runtime.clone()).await;
    let calls = runtime.sent_update_calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].method, "icrc1_balance_of");
    assert_eq!(
        calls[0].single_arg::<Account>(),
        operating_top_up_account(TEST_CANISTER_ID)
    );
    assert_eq!(read_state(|s| s.operating_balance().balance()), 0);
    assert!(read_state(|s| s.operating_balance().pending()).is_none());
    let expected = replay();
    read_state(|s| assert_eq!(s, &expected));
}

#[tokio::test]
async fn should_retry_identical_burn_after_lost_reply_and_upgrade_and_credit_duplicate_once() {
    setup();
    let first = runtime()
        .add_stub_response(Nat::from(TARGET))
        .add_stub_error(IcError::CallPerformFailed);
    refill_operating_balance(first.clone()).await;
    assert_eq!(read_state(|s| s.operating_balance().balance()), 0);
    let initial_intent = pending();
    let initial_args = first.sent_update_calls()[1].single_arg::<TransferArg>();
    let restored = replay();
    reset_state();
    init_once_state(restored);
    // Disabling stops new intents, but still resolves the previously issued burn.
    configure(OperatingRefillConfig {
        enabled: false,
        ..Default::default()
    });
    let retry = runtime().add_stub_response(Err::<Nat, TransferError>(TransferError::Duplicate {
        duplicate_of: Nat::from(42u8),
    }));
    refill_operating_balance(retry.clone()).await;
    assert_eq!(retry.sent_update_calls().len(), 1);
    assert_eq!(
        retry.sent_update_calls()[0].single_arg::<TransferArg>(),
        initial_args
    );
    assert_eq!(read_state(|s| s.operating_balance().balance()), TARGET);
    assert_eq!(initial_intent.amount, TARGET);
    assert!(read_state(|s| s.operating_balance().pending()).is_none());
    let idle = TestCanisterRuntime::new();
    refill_operating_balance(idle.clone()).await;
    assert!(idle.sent_update_calls().is_empty());
    assert_eq!(read_state(|s| s.operating_balance().completed_refills()), 1);
    let expected = replay();
    read_state(|s| assert_eq!(s, &expected));
}

#[tokio::test]
async fn should_retain_intent_on_transient_errors() {
    for error in [
        TransferError::TemporarilyUnavailable,
        TransferError::CreatedInFuture { ledger_time: 2_000 },
        TransferError::GenericError {
            error_code: 1u8.into(),
            message: "retry".into(),
        },
    ] {
        setup();
        let first = runtime()
            .add_stub_response(Nat::from(TARGET))
            .add_stub_response(Err::<Nat, TransferError>(error));
        refill_operating_balance(first.clone()).await;
        let before = pending();
        let retry = runtime().add_stub_error(IcError::CallPerformFailed);
        refill_operating_balance(retry.clone()).await;
        assert_eq!(pending(), before);
        assert_eq!(retry.sent_update_calls().len(), 1);
        assert_eq!(read_state(|s| s.operating_balance().balance()), 0);
    }
}

#[tokio::test]
async fn should_clear_first_attempt_rejections_and_use_a_new_refill_id() {
    for error in [
        TransferError::InsufficientFunds {
            balance: 0u8.into(),
        },
        TransferError::BadBurn {
            min_burn_amount: (TARGET + 1).into(),
        },
        TransferError::BadFee {
            expected_fee: 1u8.into(),
        },
    ] {
        setup();
        let rejections = errors(OperatingRefillErrorReason::Rejected);
        let rejected = runtime()
            .add_stub_response(Nat::from(TARGET))
            .add_stub_response(Err::<Nat, TransferError>(error));
        refill_operating_balance(rejected).await;
        assert_eq!(errors(OperatingRefillErrorReason::Rejected), rejections + 1);
        assert!(read_state(|s| s.operating_balance().pending()).is_none());
        assert_eq!(read_state(|s| s.operating_balance().balance()), 0);
        assert_eq!(read_state(|s| s.operating_balance().failed_refills()), 1);
        let next = runtime()
            .add_stub_response(Nat::from(TARGET))
            .add_stub_response(Ok::<Nat, TransferError>(42u8.into()));
        refill_operating_balance(next.clone()).await;
        let args = next.sent_update_calls()[1].single_arg::<TransferArg>();
        assert_eq!(
            args.memo,
            Some(Memo::from(BurnMemo::OperatingRefill { refill_id: 1 }).into())
        );
        assert_eq!(read_state(|s| s.operating_balance().balance()), TARGET);
    }
}

#[tokio::test]
async fn should_quarantine_a_rejection_after_unknown_outcome() {
    setup();
    let first = runtime()
        .add_stub_response(Nat::from(TARGET))
        .add_stub_error(IcError::CallPerformFailed);
    refill_operating_balance(first).await;
    let retry = runtime().add_stub_response(Err::<Nat, TransferError>(
        TransferError::InsufficientFunds {
            balance: 0u8.into(),
        },
    ));
    refill_operating_balance(retry).await;
    assert!(pending().quarantined);
    assert_eq!(
        errors(OperatingRefillErrorReason::RejectedAfterUnknownOutcome),
        1
    );
    assert_metrics(&[
        "operating_refill_pending 1 ",
        "operating_refill_quarantined 1 ",
    ]);
    assert_eq!(read_state(|s| s.operating_balance().balance()), 0);
    let idle = TestCanisterRuntime::new();
    refill_operating_balance(idle.clone()).await;
    assert!(idle.sent_update_calls().is_empty());
}

#[tokio::test]
async fn should_quarantine_expired_or_too_old_burns_without_new_arguments() {
    for expires_locally in [true, false] {
        setup();
        let first = runtime()
            .add_stub_response(Nat::from(TARGET))
            .add_stub_error(IcError::CallPerformFailed);
        refill_operating_balance(first).await;
        let intent = pending();
        let reason = if expires_locally {
            OperatingRefillErrorReason::DeduplicationExpired
        } else {
            OperatingRefillErrorReason::TooOld
        };
        let quarantines = errors(reason);
        let retry = if expires_locally {
            TestCanisterRuntime::new().with_increasing_time_from(
                intent.created_at_time + LEDGER_DEDUPLICATION_WINDOW.as_nanos() as u64 + 1,
            )
        } else {
            runtime().add_stub_response(Err::<Nat, TransferError>(TransferError::TooOld))
        };
        refill_operating_balance(retry.clone()).await;
        assert_eq!(
            retry.sent_update_calls().len(),
            usize::from(!expires_locally)
        );
        assert_eq!(errors(reason), quarantines + 1);
        assert_eq!(
            pending(),
            PendingOperatingRefill {
                quarantined: true,
                ..intent
            }
        );
        assert_eq!(read_state(|s| s.operating_balance().balance()), 0);
        let expected = replay();
        read_state(|s| assert_eq!(s, &expected));
    }
}

#[tokio::test]
async fn should_prevent_concurrent_refill_calls() {
    setup();
    let runtime = runtime()
        .add_stub_response(Nat::from(TARGET))
        .add_stub_response(Ok::<Nat, TransferError>(42u8.into()));
    let ((), ()) = tokio::join!(
        refill_operating_balance(runtime.clone()),
        refill_operating_balance(runtime.clone())
    );
    assert_eq!(runtime.sent_update_calls().len(), 2);
    assert_eq!(read_state(|s| s.operating_balance().completed_refills()), 1);
}

#[tokio::test]
async fn should_preserve_settings_and_funds_when_the_balance_read_fails() {
    setup();
    let before = read_state(Clone::clone);
    let failed = runtime().add_stub_error(IcError::CallPerformFailed);
    refill_operating_balance(failed).await;
    read_state(|s| assert_eq!(s, &before));
    assert_eq!(top_up_balance_read().map(|read| read.balance), None);
    assert_eq!(errors(OperatingRefillErrorReason::BalanceRead), 1);
}

#[tokio::test]
async fn should_monitor_top_up_funding_even_when_no_refill_is_due() {
    setup();
    let first = runtime()
        .add_stub_response(Nat::from(TARGET))
        .add_stub_response(Ok::<Nat, TransferError>(42u8.into()));
    refill_operating_balance(first).await;
    let poll = runtime().add_stub_response(Nat::from(123u8));
    refill_operating_balance(poll.clone()).await;
    assert_eq!(poll.sent_update_calls().len(), 1);
    assert_eq!(top_up_balance_read().map(|read| read.balance), Some(123));
    assert_eq!(read_state(|s| s.operating_balance().balance()), TARGET);
    let mut encoder = ic_metrics_encoder::MetricsEncoder::new(Vec::new(), 0);
    read_state(|s| crate::metrics::encode_operating_balance_metrics(&mut encoder, s)).unwrap();
    let metrics = String::from_utf8(encoder.into_inner()).unwrap();
    assert!(metrics.contains("operating_top_up_underfunded 1 "));
}

#[tokio::test]
async fn should_recheck_configuration_after_balance_read() {
    setup();
    let runtime = runtime().add_stub_response(Nat::from(TARGET));
    // The recording runtime yields at the call boundary, as the IC does.
    let disable = async {
        configure(OperatingRefillConfig {
            enabled: false,
            ..Default::default()
        });
    };
    tokio::join!(refill_operating_balance(runtime.clone()), disable);
    assert_eq!(runtime.sent_update_calls().len(), 1);
    assert_eq!(read_state(|s| s.operating_balance().next_refill_id()), 0);
}

#[tokio::test]
async fn should_persist_intent_before_burn_and_recover_an_interrupted_attempt() {
    setup();
    let first = runtime().add_stub_response(Nat::from(TARGET));
    let mut work = Box::pin(refill_operating_balance(first.clone()));
    assert!(futures::poll!(work.as_mut()).is_pending()); // Waiting for the balance read.
    assert!(read_state(|s| s.operating_balance().pending()).is_none());
    assert!(futures::poll!(work.as_mut()).is_pending()); // Waiting for the burn reply.
    assert_eq!(first.sent_update_calls().len(), 2);
    let intent = pending();
    assert_eq!(intent.amount, TARGET);
    // Interruption leaves the durable intent intact, as does a callback trap.
    drop(work);
    let restored = replay();
    reset_state();
    init_once_state(restored);
    let retry = runtime().add_stub_response(Err::<Nat, TransferError>(TransferError::Duplicate {
        duplicate_of: 42u8.into(),
    }));
    refill_operating_balance(retry.clone()).await;
    assert_eq!(
        retry.sent_update_calls()[0].single_arg::<TransferArg>(),
        first.sent_update_calls()[1].single_arg::<TransferArg>()
    );
    assert_eq!(read_state(|s| s.operating_balance().balance()), TARGET);
}

#[tokio::test]
async fn should_not_start_a_burn_for_a_balance_that_cannot_fit_u64() {
    setup();
    let oversized = Nat::from(u64::MAX) + Nat::from(1u8);
    let runtime = runtime().add_stub_response(oversized);
    refill_operating_balance(runtime.clone()).await;
    assert_eq!(runtime.sent_update_calls().len(), 1);
    assert!(read_state(|s| s.operating_balance().pending()).is_none());
    assert_eq!(read_state(|s| s.operating_balance().balance()), 0);
    assert_eq!(errors(OperatingRefillErrorReason::BalanceOverflow), 1);
}

#[tokio::test]
async fn should_expose_low_funding_and_unknown_burn_metrics() {
    setup();
    let runtime = runtime()
        .add_stub_response(Nat::from(TARGET / 2))
        .add_stub_error(IcError::CallPerformFailed);
    refill_operating_balance(runtime).await;
    let mut encoder = ic_metrics_encoder::MetricsEncoder::new(Vec::new(), 0);
    read_state(|s| crate::metrics::encode_operating_balance_metrics(&mut encoder, s)).unwrap();
    let metrics = String::from_utf8(encoder.into_inner()).unwrap();
    for expected in [
        "operating_balance 0 ",
        "operating_refill_pending 1 ",
        "operating_refill_quarantined 0 ",
        "operating_balance_low 1 ",
        "operating_top_up_balance_known 1 ",
        "operating_top_up_underfunded 1 ",
        "operating_refill_errors{reason=\"call_error\"} 1 ",
        "operating_refill_errors{reason=\"rejected\"} 0 ",
    ] {
        assert!(
            metrics.contains(expected),
            "missing {expected} in {metrics}"
        );
    }
}

#[tokio::test]
async fn should_refill_with_default_thresholds_without_configuration() {
    init_state();
    let runtime = runtime()
        .add_stub_response(Nat::from(200_000_000u64))
        .add_stub_response(Ok::<Nat, TransferError>(42u8.into()));
    refill_operating_balance(runtime.clone()).await;
    assert_eq!(runtime.sent_update_calls().len(), 2);
    assert_eq!(
        runtime.sent_update_calls()[1]
            .single_arg::<TransferArg>()
            .amount,
        Nat::from(100_000_000u64)
    );
    assert_eq!(read_state(|s| s.operating_balance().balance()), 100_000_000);
}

#[tokio::test]
async fn should_check_default_funding_without_burning_an_empty_account() {
    init_state();
    let runtime = runtime().add_stub_response(Nat::from(0u8));
    refill_operating_balance(runtime.clone()).await;
    assert_eq!(runtime.sent_update_calls().len(), 1);
    assert_eq!(runtime.sent_update_calls()[0].method, "icrc1_balance_of");
    assert_eq!(read_state(|s| s.operating_balance().balance()), 0);
    assert!(read_state(|s| s.operating_balance().pending()).is_none());
}

#[tokio::test]
async fn should_allow_refills_below_one_solana_signature_fee() {
    // Exercise both the funding cap and a configured target below 5,000 lamports.
    for small_target in [false, true] {
        setup();
        let amount = if small_target { 4_000 } else { 4_999 };
        if small_target {
            configure(OperatingRefillConfig {
                enabled: true,
                low_water_mark: 1_000,
                target: amount,
            });
        }
        let available = if small_target { TARGET } else { amount };
        let runtime = runtime()
            .add_stub_response(Nat::from(available))
            .add_stub_response(Ok::<Nat, TransferError>(42u8.into()));
        refill_operating_balance(runtime.clone()).await;
        assert_eq!(runtime.sent_update_calls().len(), 2);
        assert_eq!(
            runtime.sent_update_calls()[1]
                .single_arg::<TransferArg>()
                .amount,
            Nat::from(amount)
        );
        assert_eq!(read_state(|s| s.operating_balance().balance()), amount);
        assert!(read_state(|s| s.operating_balance().pending()).is_none());
    }
}

#[tokio::test]
async fn should_keep_the_last_read_top_up_balance_until_the_next_read() {
    setup();
    let runtime = runtime()
        .add_stub_response(Nat::from(TARGET + 7))
        .add_stub_response(Ok::<Nat, TransferError>(42u8.into()));
    refill_operating_balance(runtime).await;
    assert_eq!(
        top_up_balance_read().map(|read| read.balance),
        Some(TARGET + 7)
    );
    let read_at = top_up_balance_read().map(|read| read.read_at);
    let next_read = TestCanisterRuntime::new()
        .with_increasing_time_from(10_000)
        .add_stub_response(Nat::from(7u8));
    refill_operating_balance(next_read).await;
    assert_eq!(top_up_balance_read().map(|read| read.balance), Some(7));
    assert!(top_up_balance_read().map(|read| read.read_at) > read_at);
}

#[tokio::test]
async fn should_report_funding_alerts_only_while_refills_are_enabled() {
    setup();
    let empty = runtime().add_stub_response(Nat::from(0u8));
    refill_operating_balance(empty).await;
    assert_metrics(&[
        "operating_balance_low 1 ",
        "operating_top_up_underfunded 1 ",
    ]);
    configure(OperatingRefillConfig {
        enabled: false,
        low_water_mark: 100_000,
        target: TARGET,
    });
    assert_metrics(&[
        "operating_balance_low 0 ",
        "operating_top_up_underfunded 0 ",
    ]);
}

#[tokio::test]
async fn should_report_a_top_up_balance_equal_to_the_target_as_funded() {
    setup();
    let runtime = runtime()
        .add_stub_response(Nat::from(TARGET))
        .add_stub_response(Ok::<Nat, TransferError>(42u8.into()));
    refill_operating_balance(runtime).await;
    assert_eq!(top_up_balance_read().map(|read| read.balance), Some(TARGET));
    assert_metrics(&[
        "operating_top_up_underfunded 0 ",
        "operating_refills_completed 1 ",
        "operating_refills_failed 0 ",
    ]);
}
