use super::credit_finalized_sweeps;
use crate::{
    constants::MAX_CONCURRENT_RPC_CALLS,
    deposit::sweep::deposit_status,
    state::{event::EventType, read_state, reset_state},
    storage::{FailedCreditReason, failed_credit_attempt_count, reset_events},
    test_fixtures::{
        EventsAssert, GetTransactionResult, account, devnet_sweep,
        events::{queue, submit_sweep_to, succeed_transaction},
        init_state,
        runtime::TestCanisterRuntime,
        signature,
    },
};
use cksol_types::{DepositSolId, DepositSolStatus};
use sol_rpc_types::Lamport;
use solana_signature::Signature;
use solana_transaction_status_client_types::{
    EncodedConfirmedTransactionWithStatusMeta, EncodedTransaction, TransactionBinaryEncoding,
};

const FRESH_SWEEP_SIGNATURE_INDEX: usize = 0x80;

#[tokio::test]
async fn should_do_nothing_without_finalized_deposits() {
    setup();

    let run_again = credit_finalized_sweeps(&TestCanisterRuntime::new()).await;

    assert!(!run_again);
    EventsAssert::assert_no_events_recorded();
}

#[tokio::test]
async fn should_ask_for_another_round_only_after_crediting_with_sweeps_left_over() {
    const SWEEPABLE_AMOUNT: Lamport = 25_000_000;
    let cases = [
        (
            "the devnet sweep was credited",
            transaction_response(devnet_sweep::derived_outcome()),
            true,
        ),
        (
            "no sweep was credited",
            GetTransactionResult::Consistent(Ok(None)),
            false,
        ),
    ];

    for (name, devnet_sweep_response, expected_run_again) in cases {
        setup();
        finalize_devnet_sweep();
        let mut runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .expect_get_transaction(devnet_sweep::transaction_signature(), devnet_sweep_response);
        for index in 0..MAX_CONCURRENT_RPC_CALLS {
            let deposit_id = (devnet_sweep::DEPOSITS.len() + index) as DepositSolId;
            let account = account(deposit_id as usize + 1);
            queue(
                deposit_id,
                devnet_sweep::fresh_deposit(account, SWEEPABLE_AMOUNT),
            );
            let sweep_signature = signature(FRESH_SWEEP_SIGNATURE_INDEX + index);
            submit_sweep_to(
                sweep_signature,
                vec![deposit_id],
                devnet_sweep::minter_main_address(),
            );
            succeed_transaction(sweep_signature);
            if index + 1 < MAX_CONCURRENT_RPC_CALLS {
                runtime = runtime.expect_get_transaction(
                    sweep_signature,
                    GetTransactionResult::Consistent(Ok(None)),
                );
            }
        }

        let run_again = credit_finalized_sweeps(&runtime).await;

        assert_eq!(run_again, expected_run_again, "{name}");
        read_state(|state| {
            let credited = if expected_run_again {
                devnet_sweep::DEPOSITS.len()
            } else {
                0
            };
            assert_eq!(state.deposits().pending_mints().len(), credited, "{name}");
            assert_eq!(
                state.deposits().finalized().len(),
                MAX_CONCURRENT_RPC_CALLS + 1 - usize::from(expected_run_again),
                "{name}"
            );
        });
    }
}

#[tokio::test]
async fn should_credit_the_amount_received_by_the_main_account() {
    setup();
    let sweep_signature = finalize_devnet_sweep();

    credit_finalized_sweeps(&runtime_returning(
        sweep_signature,
        devnet_sweep::derived_outcome(),
    ))
    .await;

    EventsAssert::from_recorded().expect_contains_event_eq(EventType::CreditedSweep {
        signature: sweep_signature,
        amount_received: devnet_sweep::AMOUNT_RECEIVED,
        mints: devnet_sweep::mints(),
    });
    read_state(|state| {
        assert!(state.deposits().finalized().is_empty());
        assert_eq!(
            state.deposits().pending_mints().len(),
            devnet_sweep::DEPOSITS.len()
        );
    });
    for reason in FailedCreditReason::ALL {
        assert_eq!(failed_credit_attempt_count(reason), 0, "{reason:?}");
    }
}

#[tokio::test]
async fn should_keep_deposits_finalized_until_the_outcome_can_be_read() {
    type Response = fn() -> GetTransactionResult;
    let cases: [(&str, Response, FailedCreditReason); 4] = [
        (
            "the transaction is not returned",
            || GetTransactionResult::Consistent(Ok(None)),
            FailedCreditReason::NotFound,
        ),
        (
            "fetching the transaction fails",
            || GetTransactionResult::Inconsistent(vec![]),
            FailedCreditReason::RpcError,
        ),
        (
            "the providers agree on a transaction that cannot be decoded",
            || {
                let mut outcome = devnet_sweep::derived_outcome();
                outcome.transaction.transaction = EncodedTransaction::Binary(
                    "not a transaction".to_string(),
                    TransactionBinaryEncoding::Base64,
                );
                transaction_response(outcome)
            },
            FailedCreditReason::InvalidResponse,
        ),
        (
            "the metadata cannot be read",
            || {
                let mut outcome = devnet_sweep::derived_outcome();
                outcome.transaction.meta = None;
                transaction_response(outcome)
            },
            FailedCreditReason::Unreadable,
        ),
    ];

    for (name, response, expected_reason) in cases {
        setup();
        let sweep_signature = finalize_devnet_sweep();
        let events_before = EventsAssert::from_recorded();
        let runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .expect_get_transaction(sweep_signature, response());

        credit_finalized_sweeps(&runtime).await;

        assert_eq!(events_before, EventsAssert::from_recorded(), "{name}");
        read_state(|state| {
            assert_eq!(
                state.deposits().finalized().deposit_count(),
                devnet_sweep::DEPOSITS.len(),
                "{name}"
            );
            assert!(state.deposits().pending_mints().is_empty(), "{name}");
            assert!(state.deposits().quarantined().is_empty(), "{name}");
        });
        assert_eq!(
            deposit_status(0),
            DepositSolStatus::Finalized {
                signature: sweep_signature.into()
            },
            "{name}"
        );
        assert_eq!(failed_credit_attempt_count(expected_reason), 1, "{name}");
    }
}

#[tokio::test]
async fn should_quarantine_deposits_if_the_outcome_does_not_match_the_plan() {
    setup();
    let sweep_signature = finalize_devnet_sweep();
    let mut outcome = devnet_sweep::derived_outcome();
    devnet_sweep::set_balances(&mut outcome, devnet_sweep::minter_main_address(), 1, 0);

    credit_finalized_sweeps(&runtime_returning(sweep_signature, outcome)).await;

    EventsAssert::from_recorded().expect_contains_event_eq(EventType::QuarantinedSweep {
        signature: sweep_signature,
    });
    read_state(|state| {
        assert!(state.deposits().finalized().is_empty());
        assert!(state.deposits().pending_mints().is_empty());
        assert_eq!(
            state.deposits().quarantined().len(),
            devnet_sweep::DEPOSITS.len()
        );
    });
    assert_eq!(
        deposit_status(0),
        DepositSolStatus::Quarantined {
            signature: sweep_signature.into()
        }
    );
    assert_eq!(failed_credit_attempt_count(FailedCreditReason::Mismatch), 1);
}

fn setup() {
    reset_state();
    reset_events();
    init_state();
    devnet_sweep::init_master_key();
}

/// Queues the deposits of the devnet sweep, submits it and finalizes it.
fn finalize_devnet_sweep() -> Signature {
    let deposits = devnet_sweep::derived_deposits();
    for (deposit_id, deposit) in &deposits {
        queue(*deposit_id, *deposit);
    }
    let sweep_signature = devnet_sweep::transaction_signature();
    submit_sweep_to(
        sweep_signature,
        deposits.iter().map(|(deposit_id, _)| *deposit_id).collect(),
        devnet_sweep::minter_main_address(),
    );
    succeed_transaction(sweep_signature);
    sweep_signature
}

fn runtime_returning(
    sweep_signature: Signature,
    outcome: EncodedConfirmedTransactionWithStatusMeta,
) -> TestCanisterRuntime {
    TestCanisterRuntime::new()
        .with_increasing_time()
        .expect_get_transaction(sweep_signature, transaction_response(outcome))
}

fn transaction_response(
    outcome: EncodedConfirmedTransactionWithStatusMeta,
) -> GetTransactionResult {
    GetTransactionResult::Consistent(Ok(Some(
        outcome.try_into().expect("failed to convert transaction"),
    )))
}
