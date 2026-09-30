use super::credit_finalized_sweeps;
use crate::{
    constants::{FEE_PER_SIGNATURE, MAX_CONCURRENT_RPC_CALLS},
    deposit::sweep::deposit_status,
    state::{
        Sweep,
        event::{CreditedDeposit, EventType},
        read_state, reset_state,
    },
    storage::reset_events,
    test_fixtures::{
        EventsAssert, GetTransactionResult, MINTER_ADDRESS, account,
        events::{queue_deposit, submit_sweep, succeed_transaction},
        init_schnorr_master_key, init_state,
        runtime::TestCanisterRuntime,
        signature,
        sweep_outcome::{MAIN_BALANCE, SweepOutcome},
    },
};
use cksol_types::{DepositSolId, DepositSolStatus};
use sol_rpc_types::Lamport;
use solana_signature::Signature;
use solana_transaction_status_client_types::EncodedConfirmedTransactionWithStatusMeta;

const SWEEPABLE_AMOUNTS: [Lamport; 3] = [30_000_000, 20_000_000, 10_000_000];
const ASSUMED_FEE: Lamport = FEE_PER_SIGNATURE * SWEEPABLE_AMOUNTS.len() as u64;
const SWEEP_SIGNATURE_INDEX: usize = 0xAA;

#[tokio::test]
async fn should_do_nothing_without_finalized_deposits() {
    setup();

    let run_again = credit_finalized_sweeps(&TestCanisterRuntime::new()).await;

    assert!(!run_again);
    EventsAssert::assert_no_events_recorded();
}

#[tokio::test]
async fn should_ask_for_another_round_when_sweeps_are_left_over() {
    const SWEEPABLE_AMOUNT: Lamport = 25_000_000;
    setup();
    let sweeps = MAX_CONCURRENT_RPC_CALLS + 1;
    let mut runtime = TestCanisterRuntime::new().with_increasing_time();
    for index in 0..sweeps {
        let deposit_id = index as DepositSolId;
        queue_deposit(deposit_id, account(index), SWEEPABLE_AMOUNT);
        let sweep_signature = signature(index);
        submit_sweep(sweep_signature, vec![deposit_id]);
        succeed_transaction(sweep_signature);
        if index < MAX_CONCURRENT_RPC_CALLS {
            let outcome = SweepOutcome::of(&finalized_sweep(sweep_signature)).encode();
            runtime = runtime.add_stub_response(transaction_response(outcome));
        }
    }

    let run_again = credit_finalized_sweeps(&runtime).await;

    assert!(run_again);
    read_state(|state| {
        assert_eq!(
            state.deposits().pending_mints().len(),
            MAX_CONCURRENT_RPC_CALLS
        );
        assert_eq!(state.deposits().finalized().deposit_count(), 1);
    });
}

#[tokio::test]
async fn should_wait_for_the_timer_when_no_sweep_was_credited() {
    const SWEEPABLE_AMOUNT: Lamport = 25_000_000;
    setup();
    let sweeps = MAX_CONCURRENT_RPC_CALLS + 1;
    let mut runtime = TestCanisterRuntime::new().with_increasing_time();
    for index in 0..sweeps {
        let deposit_id = index as DepositSolId;
        queue_deposit(deposit_id, account(index), SWEEPABLE_AMOUNT);
        let sweep_signature = signature(index);
        submit_sweep(sweep_signature, vec![deposit_id]);
        succeed_transaction(sweep_signature);
        if index < MAX_CONCURRENT_RPC_CALLS {
            runtime = runtime.add_stub_response(GetTransactionResult::Consistent(Ok(None)));
        }
    }

    let run_again = credit_finalized_sweeps(&runtime).await;

    assert!(!run_again);
    read_state(|state| {
        assert!(state.deposits().pending_mints().is_empty());
        assert_eq!(state.deposits().finalized().len(), sweeps);
    });
}

#[tokio::test]
async fn should_credit_the_amount_received_by_the_main_account() {
    setup();
    let sweep_signature = queue_finalized_sweep();
    let outcome = SweepOutcome::of(&finalized_sweep(sweep_signature)).encode();

    credit_finalized_sweeps(&runtime_returning(outcome)).await;

    EventsAssert::from_recorded().expect_contains_event_eq(EventType::CreditedSweep {
        signature: sweep_signature,
        amount_received: SWEEPABLE_AMOUNTS.iter().sum::<Lamport>() - ASSUMED_FEE,
        mints: SWEEPABLE_AMOUNTS
            .iter()
            .enumerate()
            .map(|(deposit_id, sweepable_amount)| CreditedDeposit {
                deposit_id: deposit_id as DepositSolId,
                amount_to_mint: sweepable_amount - FEE_PER_SIGNATURE,
            })
            .collect(),
    });
    read_state(|state| {
        assert!(state.deposits().finalized().is_empty());
        assert_eq!(
            state.deposits().pending_mints().len(),
            SWEEPABLE_AMOUNTS.len()
        );
    });
}

#[tokio::test]
async fn should_keep_deposits_finalized_until_the_outcome_can_be_read() {
    type Response = fn(&Sweep) -> GetTransactionResult;
    let cases: [(&str, Response); 3] = [
        ("the transaction is not returned", |_| {
            GetTransactionResult::Consistent(Ok(None))
        }),
        ("fetching the transaction fails", |_| {
            GetTransactionResult::Inconsistent(vec![])
        }),
        ("the metadata cannot be read", |sweep| {
            transaction_response(SweepOutcome::of(sweep).encode_without_meta())
        }),
    ];

    for (name, response) in cases {
        setup();
        let sweep_signature = queue_finalized_sweep();
        let events_before = EventsAssert::from_recorded();
        let runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .add_stub_response(response(&finalized_sweep(sweep_signature)));

        credit_finalized_sweeps(&runtime).await;

        assert_eq!(events_before, EventsAssert::from_recorded(), "{name}");
        read_state(|state| {
            assert_eq!(
                state.deposits().finalized().deposit_count(),
                SWEEPABLE_AMOUNTS.len(),
                "{name}"
            );
            assert!(state.deposits().pending_mints().is_empty(), "{name}");
        });
        assert_eq!(
            deposit_status(0),
            DepositSolStatus::Finalized {
                signature: sweep_signature.into()
            },
            "{name}"
        );
    }
}

#[tokio::test]
async fn should_keep_deposits_finalized_if_the_outcome_does_not_match_the_plan() {
    setup();
    let sweep_signature = queue_finalized_sweep();
    let events_before = EventsAssert::from_recorded();
    let outcome = SweepOutcome::of(&finalized_sweep(sweep_signature))
        .with_balances(MINTER_ADDRESS, MAIN_BALANCE, MAIN_BALANCE - 1)
        .encode();

    credit_finalized_sweeps(&runtime_returning(outcome)).await;

    assert_eq!(events_before, EventsAssert::from_recorded());
    read_state(|state| {
        assert_eq!(
            state.deposits().finalized().deposit_count(),
            SWEEPABLE_AMOUNTS.len()
        );
        assert!(state.deposits().pending_mints().is_empty());
    });
    assert_eq!(
        deposit_status(0),
        DepositSolStatus::Finalized {
            signature: sweep_signature.into()
        }
    );
}

fn setup() {
    reset_state();
    reset_events();
    init_state();
    init_schnorr_master_key();
}

fn queue_finalized_sweep() -> Signature {
    for (deposit_id, sweepable_amount) in SWEEPABLE_AMOUNTS.iter().enumerate() {
        queue_deposit(
            deposit_id as DepositSolId,
            account(deposit_id),
            *sweepable_amount,
        );
    }
    let sweep_signature = signature(SWEEP_SIGNATURE_INDEX);
    submit_sweep(
        sweep_signature,
        (0..SWEEPABLE_AMOUNTS.len() as DepositSolId).collect(),
    );
    succeed_transaction(sweep_signature);
    sweep_signature
}

fn finalized_sweep(signature: Signature) -> Sweep {
    read_state(|state| {
        state
            .deposits()
            .finalized()
            .get(&signature)
            .cloned()
            .expect("the sweep should be finalized")
    })
}

fn runtime_returning(outcome: EncodedConfirmedTransactionWithStatusMeta) -> TestCanisterRuntime {
    TestCanisterRuntime::new()
        .with_increasing_time()
        .add_stub_response(transaction_response(outcome))
}

fn transaction_response(
    outcome: EncodedConfirmedTransactionWithStatusMeta,
) -> GetTransactionResult {
    GetTransactionResult::Consistent(Ok(Some(
        outcome.try_into().expect("failed to convert transaction"),
    )))
}
