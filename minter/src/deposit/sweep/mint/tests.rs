use super::process_pending_mints;
use crate::{
    constants::{
        FEE_PER_SIGNATURE, GET_BALANCE_CYCLES, LEDGER_DEDUPLICATION_WINDOW,
        MAX_CONCURRENT_RPC_CALLS,
    },
    deposit::sweep::{deposit_sol, timer::MAX_DEPOSITS_PER_SWEEP},
    state::{TaskType, event::EventType, mutate_state, read_state, reset_state},
    storage::reset_events,
    test_fixtures::{
        BLOCK_INDEX, DEPOSIT_SOL_REQUIRED_CYCLES, EventsAssert, MINIMUM_DEPOSIT_AMOUNT, account,
        deposit_address,
        events::{credit_sweep, credit_sweep_at, queue_deposit, submit_sweep, succeed_transaction},
        init_schnorr_master_key, init_state,
        runtime::{CallResponse, TestCanisterRuntime},
        signature,
    },
};
use candid::Nat;
use cksol_types::{DepositSolError, DepositSolId, DepositSolStatus, Memo, MintMemo};
use ic_canister_runtime::IcError;
use icrc_ledger_types::icrc1::{
    account::Account,
    transfer::{BlockIndex, NumTokens, TransferArg, TransferError},
};
use sol_rpc_types::{Lamport, MultiRpcResult};
use solana_signature::Signature;

type MintResult = Result<BlockIndex, TransferError>;

const SWEEP_SIGNATURE_INDEX: usize = 0xAA;
const SWEEPABLE_AMOUNT: Lamport = 25_000_000;
const MINTED_AMOUNT: Lamport = SWEEPABLE_AMOUNT - FEE_PER_SIGNATURE;
const CREDITED_AT_TIME: u64 = 1_234;

#[tokio::test]
async fn should_mint_pending_deposit_and_release_the_account() {
    setup();
    let sweep_signature = credit_sweep_of_deposit_zero();
    let runtime = mint_runtime([(
        expected_transfer_arg(sweep_signature),
        Ok(BLOCK_INDEX.into()),
    )]);

    process_pending_mints(runtime.clone()).await;

    assert_eq!(
        deposit_status(0),
        DepositSolStatus::Minted {
            block_index: BLOCK_INDEX,
            minted_amount: MINTED_AMOUNT,
        }
    );
    EventsAssert::from_recorded().expect_contains_event_eq(EventType::MintedSweptDeposit {
        deposit_id: 0,
        mint_block_index: BLOCK_INDEX.into(),
    });
    assert_eq!(runtime.set_timer_call_count(), 0);

    let new_deposit_id = deposit_sol(&deposit_sol_runtime(account(1)), account(1)).await;
    assert_eq!(new_deposit_id, Ok(1));
}

#[tokio::test]
async fn should_record_duplicate_reply_as_minted() {
    setup();
    let sweep_signature = credit_sweep_of_deposit_zero();
    let runtime = mint_runtime([(
        expected_transfer_arg(sweep_signature),
        Err(TransferError::Duplicate {
            duplicate_of: BlockIndex::from(BLOCK_INDEX),
        }),
    )]);

    process_pending_mints(runtime).await;

    assert_eq!(
        deposit_status(0),
        DepositSolStatus::Minted {
            block_index: BLOCK_INDEX,
            minted_amount: MINTED_AMOUNT,
        }
    );
    EventsAssert::from_recorded().expect_contains_event_eq(EventType::MintedSweptDeposit {
        deposit_id: 0,
        mint_block_index: BLOCK_INDEX.into(),
    });
}

#[tokio::test]
async fn should_retry_after_transient_failure_with_exactly_the_same_arguments() {
    let transient_failures: Vec<(&str, CallResponse<MintResult>)> = vec![
        (
            "the ledger is temporarily unavailable",
            CallResponse::Reply(Err(TransferError::TemporarilyUnavailable)),
        ),
        (
            "the ledger returns a generic error",
            CallResponse::Reply(Err(TransferError::GenericError {
                error_code: Nat::from(42_u8),
                message: "out of luck".to_string(),
            })),
        ),
        (
            "the minter clock is ahead of the ledger",
            CallResponse::Reply(Err(TransferError::CreatedInFuture { ledger_time: 0 })),
        ),
        (
            "the call to the ledger fails",
            CallResponse::Failed(IcError::CallPerformFailed),
        ),
    ];

    for (name, failure) in transient_failures {
        setup();
        let sweep_signature = credit_sweep_of_deposit_zero();
        let events_before = EventsAssert::from_recorded();
        let failing_runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .expect_icrc1_transfer(expected_transfer_arg(sweep_signature), failure);

        process_pending_mints(failing_runtime.clone()).await;

        assert_eq!(
            deposit_status(0),
            DepositSolStatus::Finalized {
                signature: sweep_signature.into()
            },
            "{name}"
        );
        assert_eq!(events_before, EventsAssert::from_recorded(), "{name}");

        let retrying_runtime = mint_runtime([(
            expected_transfer_arg(sweep_signature),
            Ok(BLOCK_INDEX.into()),
        )]);

        process_pending_mints(retrying_runtime.clone()).await;

        assert_eq!(
            deposit_status(0),
            DepositSolStatus::Minted {
                block_index: BLOCK_INDEX,
                minted_amount: MINTED_AMOUNT,
            },
            "{name}"
        );
    }
}

#[tokio::test]
async fn should_quarantine_stale_pending_mint_without_calling_the_ledger() {
    const CREDITED_AT: u64 = 1_000;
    setup();
    queue_deposit(0, account(1), SWEEPABLE_AMOUNT);
    let sweep_signature = signature(SWEEP_SIGNATURE_INDEX);
    submit_sweep(sweep_signature, vec![0]);
    succeed_transaction(sweep_signature);
    credit_sweep_at(sweep_signature, SWEEPABLE_AMOUNT, CREDITED_AT);
    let stale_now = CREDITED_AT + LEDGER_DEDUPLICATION_WINDOW.as_nanos() as u64 + 1;
    let runtime = TestCanisterRuntime::new().add_times([stale_now; 3]);

    process_pending_mints(runtime.clone()).await;

    assert_eq!(
        deposit_status(0),
        DepositSolStatus::Quarantined {
            signature: sweep_signature.into()
        }
    );
    EventsAssert::from_recorded()
        .expect_contains_event_eq(EventType::QuarantinedPendingMint { deposit_id: 0 });
    assert_eq!(runtime.set_timer_call_count(), 0);

    let result = deposit_sol(
        &TestCanisterRuntime::new().add_msg_cycles_available(DEPOSIT_SOL_REQUIRED_CYCLES),
        account(1),
    )
    .await;
    assert_eq!(result, Err(DepositSolError::Quarantined { deposit_id: 0 }));
}

#[tokio::test]
async fn should_quarantine_pending_mint_the_ledger_rejects_as_too_old() {
    setup();
    let sweep_signature = credit_sweep_of_deposit_zero();
    let runtime = mint_runtime([(
        expected_transfer_arg(sweep_signature),
        Err(TransferError::TooOld),
    )]);

    process_pending_mints(runtime.clone()).await;

    assert_eq!(
        deposit_status(0),
        DepositSolStatus::Quarantined {
            signature: sweep_signature.into()
        }
    );
    EventsAssert::from_recorded()
        .expect_contains_event_eq(EventType::QuarantinedPendingMint { deposit_id: 0 });
}

#[tokio::test]
async fn should_reschedule_until_all_pending_mints_are_processed() {
    const NUM_DEPOSITS: usize = MAX_CONCURRENT_RPC_CALLS + 1;
    setup();
    credit_sweeps_of_deposits(NUM_DEPOSITS);
    let runtime = mint_runtime((0..MAX_CONCURRENT_RPC_CALLS).map(|deposit_id| {
        (
            pending_mint_transfer_arg(deposit_id),
            Ok((deposit_id as u64).into()),
        )
    }));

    process_pending_mints(runtime.clone()).await;

    read_state(|s| {
        assert_eq!(s.deposits().minted().len(), MAX_CONCURRENT_RPC_CALLS);
        assert_eq!(s.deposits().pending_mints().len(), 1);
    });
    assert_eq!(runtime.set_timer_call_count(), 1);

    let runtime = mint_runtime([(
        pending_mint_transfer_arg(MAX_CONCURRENT_RPC_CALLS),
        Ok((MAX_CONCURRENT_RPC_CALLS as u64).into()),
    )]);

    process_pending_mints(runtime.clone()).await;

    read_state(|s| {
        assert_eq!(s.deposits().minted().len(), NUM_DEPOSITS);
        assert!(s.deposits().pending_mints().is_empty());
    });
    assert_eq!(runtime.set_timer_call_count(), 0);
}

#[tokio::test]
async fn should_return_early_if_task_already_active() {
    setup();
    credit_sweep_of_deposit_zero();
    let events_before = EventsAssert::from_recorded();
    mutate_state(|s| {
        s.active_tasks_mut().insert(TaskType::Mint);
    });
    let runtime = TestCanisterRuntime::new();

    process_pending_mints(runtime.clone()).await;

    assert_eq!(events_before, EventsAssert::from_recorded());
}

#[tokio::test]
async fn should_return_early_if_no_pending_mints() {
    setup();
    let runtime = TestCanisterRuntime::new();

    process_pending_mints(runtime.clone()).await;

    EventsAssert::assert_no_events_recorded();
    assert_eq!(runtime.set_timer_call_count(), 0);
}

#[tokio::test]
async fn should_quarantine_pending_mint_on_deterministic_ledger_rejection() {
    let rejections = [
        TransferError::BadFee {
            expected_fee: Nat::from(10_u8),
        },
        TransferError::BadBurn {
            min_burn_amount: Nat::from(10_u8),
        },
        TransferError::InsufficientFunds {
            balance: Nat::from(0_u8),
        },
    ];

    for rejection in rejections {
        setup();
        let name = format!("{rejection:?}");
        let sweep_signature = credit_sweep_of_deposit_zero();
        let runtime = mint_runtime([(expected_transfer_arg(sweep_signature), Err(rejection))]);

        process_pending_mints(runtime.clone()).await;

        assert_eq!(
            deposit_status(0),
            DepositSolStatus::Quarantined {
                signature: sweep_signature.into()
            },
            "{name}"
        );
        EventsAssert::from_recorded()
            .expect_contains_event_eq(EventType::QuarantinedPendingMint { deposit_id: 0 });
        assert_eq!(runtime.set_timer_call_count(), 0, "{name}");
    }
}

#[tokio::test]
async fn should_not_reschedule_after_a_round_of_transient_failures() {
    const NUM_DEPOSITS: usize = MAX_CONCURRENT_RPC_CALLS + 1;
    setup();
    credit_sweeps_of_deposits(NUM_DEPOSITS);
    let runtime = mint_runtime((0..MAX_CONCURRENT_RPC_CALLS).map(|deposit_id| {
        (
            pending_mint_transfer_arg(deposit_id),
            Err(TransferError::TemporarilyUnavailable),
        )
    }));

    process_pending_mints(runtime.clone()).await;

    read_state(|s| {
        assert_eq!(s.deposits().pending_mints().len(), NUM_DEPOSITS);
        assert!(s.deposits().minted().is_empty());
    });
    assert_eq!(runtime.set_timer_call_count(), 0);
}

fn setup() {
    reset_state();
    reset_events();
    init_state();
    init_schnorr_master_key();
}

fn credit_sweep_of_deposit_zero() -> Signature {
    queue_deposit(0, account(1), SWEEPABLE_AMOUNT);
    let sweep_signature = signature(SWEEP_SIGNATURE_INDEX);
    submit_sweep(sweep_signature, vec![0]);
    succeed_transaction(sweep_signature);
    credit_sweep_at(sweep_signature, SWEEPABLE_AMOUNT, CREDITED_AT_TIME);
    sweep_signature
}

fn credit_sweeps_of_deposits(num_deposits: usize) {
    assert!(num_deposits <= 2 * MAX_DEPOSITS_PER_SWEEP);
    for deposit_id in 0..num_deposits {
        queue_deposit(
            deposit_id as DepositSolId,
            account(deposit_id + 1),
            SWEEPABLE_AMOUNT,
        );
    }
    for (sweep_index, deposit_ids) in [
        (0..num_deposits.min(MAX_DEPOSITS_PER_SWEEP)),
        (num_deposits.min(MAX_DEPOSITS_PER_SWEEP)..num_deposits),
    ]
    .into_iter()
    .enumerate()
    {
        if deposit_ids.is_empty() {
            continue;
        }
        let sweep_signature = signature(SWEEP_SIGNATURE_INDEX + sweep_index);
        let deposit_ids: Vec<DepositSolId> = deposit_ids.map(|id| id as DepositSolId).collect();
        let swept_amount = SWEEPABLE_AMOUNT * deposit_ids.len() as u64;
        submit_sweep(sweep_signature, deposit_ids);
        succeed_transaction(sweep_signature);
        credit_sweep(sweep_signature, swept_amount);
    }
}

fn mint_runtime<I>(mints: I) -> TestCanisterRuntime
where
    I: IntoIterator<Item = (TransferArg, MintResult)>,
{
    let mut runtime = TestCanisterRuntime::new().with_increasing_time();
    for (args, result) in mints {
        runtime = runtime.expect_icrc1_transfer(args, result);
    }
    runtime
}

fn deposit_sol_runtime(depositor: Account) -> TestCanisterRuntime {
    TestCanisterRuntime::new()
        .with_increasing_time()
        .expecting_charges()
        .add_msg_cycles_available(DEPOSIT_SOL_REQUIRED_CYCLES)
        .add_msg_cycles_refunded(GET_BALANCE_CYCLES / 2)
        .expect_get_balance(
            deposit_address(depositor),
            MultiRpcResult::Consistent(Ok(MINIMUM_DEPOSIT_AMOUNT)),
        )
}

/// The mint of the deposit credited by [`credit_sweep_of_deposit_zero`].
fn expected_transfer_arg(sweep_signature: Signature) -> TransferArg {
    transfer_arg(0, sweep_signature, CREDITED_AT_TIME)
}

/// The mint of one of the deposits credited by [`credit_sweeps_of_deposits`].
fn pending_mint_transfer_arg(deposit_id: usize) -> TransferArg {
    let sweep_index = deposit_id / MAX_DEPOSITS_PER_SWEEP;
    transfer_arg(
        deposit_id,
        signature(SWEEP_SIGNATURE_INDEX + sweep_index),
        0,
    )
}

fn transfer_arg(
    deposit_id: usize,
    sweep_signature: Signature,
    created_at_time: u64,
) -> TransferArg {
    TransferArg {
        from_subaccount: None,
        to: account(deposit_id + 1),
        fee: None,
        created_at_time: Some(created_at_time),
        memo: Some(Memo::from(MintMemo::convert(sweep_signature)).into()),
        amount: NumTokens::from(MINTED_AMOUNT),
    }
}

fn deposit_status(deposit_id: DepositSolId) -> DepositSolStatus {
    read_state(|s| s.deposits().status(deposit_id))
}
