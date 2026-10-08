use super::process_pending_mints;
use crate::{
    constants::{GET_BALANCE_CYCLES, LEDGER_DEDUPLICATION_WINDOW, MAX_PENDING_MINTS_PER_ROUND},
    deposit::sweep::{deposit_sol, timer::MAX_DEPOSITS_PER_SWEEP},
    state::{TaskType, event::EventType, mutate_state, read_state, reset_state},
    storage::{FailedMintReason, failed_mint_attempt_count, reset_events},
    test_fixtures::{
        BLOCK_INDEX, DEPOSIT_SOL_REQUIRED_CYCLES, EventsAssert, MINIMUM_DEPOSIT_AMOUNT, account,
        flow::deposit::{DepositFlow, PendingMintFlow, SweepFlow},
        init_schnorr_master_key, init_state,
        runtime::TestCanisterRuntime,
        signature,
    },
};
use candid::Nat;
use cksol_types::{DepositSolId, DepositSolStatus, Memo, MintMemo};
use futures::FutureExt;
use ic_canister_runtime::IcError;
use icrc_ledger_types::icrc1::transfer::{BlockIndex, NumTokens, TransferArg, TransferError};
use sol_rpc_types::{Lamport, MultiRpcResult};
use std::panic::AssertUnwindSafe;

type MintResult = Result<BlockIndex, TransferError>;

const SWEEP_SIGNATURE_INDEX: usize = 0xAA;
const SWEEPABLE_AMOUNT: Lamport = 25_000_000;
const CREDITED_AT_TIME: u64 = 1_234;

#[tokio::test]
async fn should_return_early_if_no_pending_mints() {
    init_state();
    let runtime = TestCanisterRuntime::new();

    process_pending_mints(runtime.clone()).await;

    EventsAssert::assert_no_events_recorded();
    assert_eq!(runtime.set_timer_call_count(), 0);
}

#[tokio::test]
async fn should_return_early_if_task_already_active() {
    setup();
    credited_pending_mint();
    let events_before = EventsAssert::from_recorded();
    mutate_state(|s| {
        s.active_tasks_mut().insert(TaskType::Mint);
    });
    let runtime = TestCanisterRuntime::new();

    process_pending_mints(runtime.clone()).await;

    assert_eq!(events_before, EventsAssert::from_recorded());
    assert!(runtime.sent_update_calls().is_empty());
}

#[tokio::test]
async fn should_mint_pending_deposit_and_release_the_account() {
    setup();
    let pending = credited_pending_mint();
    let runtime = mint_runtime([Ok(BLOCK_INDEX.into())]);

    process_pending_mints(runtime.clone()).await;

    assert_eq!(
        deposit_status(pending.deposit_id),
        DepositSolStatus::Minted {
            block_index: BLOCK_INDEX,
            minted_amount: pending.amount_to_mint,
        }
    );
    EventsAssert::from_recorded().expect_contains_event_eq(EventType::MintedSweptDeposit {
        deposit_id: pending.deposit_id,
        mint_block_index: BLOCK_INDEX.into(),
    });
    assert_eq!(
        transfer_args_sent_by(&runtime),
        vec![expected_transfer_arg(&pending)]
    );
    assert_eq!(runtime.set_timer_call_count(), 0);

    let new_deposit_id = deposit_sol(&deposit_sol_runtime(), pending.account).await;
    assert_eq!(new_deposit_id, Ok(pending.deposit_id + 1));
}

#[tokio::test]
async fn should_record_duplicate_reply_as_minted() {
    setup();
    let pending = credited_pending_mint();
    let runtime = mint_runtime([Err(TransferError::Duplicate {
        duplicate_of: BlockIndex::from(BLOCK_INDEX),
    })]);

    process_pending_mints(runtime).await;

    assert_eq!(
        deposit_status(pending.deposit_id),
        DepositSolStatus::Minted {
            block_index: BLOCK_INDEX,
            minted_amount: pending.amount_to_mint,
        }
    );
    EventsAssert::from_recorded().expect_contains_event_eq(EventType::MintedSweptDeposit {
        deposit_id: pending.deposit_id,
        mint_block_index: BLOCK_INDEX.into(),
    });
}

#[tokio::test]
async fn should_retry_after_transient_failure_with_exactly_the_same_arguments() {
    type AddFailure = fn(TestCanisterRuntime) -> TestCanisterRuntime;
    let transient_failures: [(&str, AddFailure, FailedMintReason); 4] = [
        (
            "the ledger is temporarily unavailable",
            |runtime| {
                runtime.add_stub_response::<MintResult>(Err(TransferError::TemporarilyUnavailable))
            },
            FailedMintReason::LedgerError,
        ),
        (
            "the ledger returns a generic error",
            |runtime| {
                runtime.add_stub_response::<MintResult>(Err(TransferError::GenericError {
                    error_code: Nat::from(42_u8),
                    message: "out of luck".to_string(),
                }))
            },
            FailedMintReason::LedgerError,
        ),
        (
            "the minter clock is ahead of the ledger",
            |runtime| {
                runtime.add_stub_response::<MintResult>(Err(TransferError::CreatedInFuture {
                    ledger_time: 0,
                }))
            },
            FailedMintReason::LedgerError,
        ),
        (
            "the call to the ledger fails",
            |runtime| runtime.add_stub_error(IcError::CallPerformFailed),
            FailedMintReason::CallError,
        ),
    ];

    let mut expected_attempts = std::collections::BTreeMap::new();
    for (name, add_failure, expected_reason) in transient_failures {
        setup();
        let pending = credited_pending_mint();
        let events_before = EventsAssert::from_recorded();
        let failing_runtime = add_failure(TestCanisterRuntime::new().with_increasing_time());

        process_pending_mints(failing_runtime.clone()).await;

        *expected_attempts.entry(expected_reason).or_insert(0_u64) += 1;
        assert_eq!(
            failed_mint_attempt_count(expected_reason),
            expected_attempts[&expected_reason],
            "{name}"
        );
        assert_eq!(
            deposit_status(pending.deposit_id),
            DepositSolStatus::Finalized {
                signature: pending.sweep_signature.into()
            },
            "{name}"
        );
        assert_eq!(events_before, EventsAssert::from_recorded(), "{name}");

        let retrying_runtime = mint_runtime([Ok(BLOCK_INDEX.into())]);

        process_pending_mints(retrying_runtime.clone()).await;

        assert_eq!(
            deposit_status(pending.deposit_id),
            DepositSolStatus::Minted {
                block_index: BLOCK_INDEX,
                minted_amount: pending.amount_to_mint,
            },
            "{name}"
        );
        let first_attempt = transfer_args_sent_by(&failing_runtime);
        let retry = transfer_args_sent_by(&retrying_runtime);
        assert_eq!(
            first_attempt,
            vec![expected_transfer_arg(&pending)],
            "{name}"
        );
        assert_eq!(first_attempt, retry, "{name}");
    }
}

#[tokio::test]
async fn should_quarantine_pending_mint() {
    struct QuarantineCase {
        name: &'static str,
        runtime: TestCanisterRuntime,
        expected_ledger_calls: usize,
        traps: bool,
        expected_reason: Option<FailedMintReason>,
    }
    let stale_now = CREDITED_AT_TIME + LEDGER_DEDUPLICATION_WINDOW.as_nanos() as u64 + 1;
    let cases = [
        QuarantineCase {
            name: "the pending mint is older than the deduplication window of the ledger",
            runtime: TestCanisterRuntime::new().add_times([stale_now; 3]),
            expected_ledger_calls: 0,
            traps: false,
            expected_reason: Some(FailedMintReason::Expired),
        },
        QuarantineCase {
            name: "the ledger rejects the mint as too old",
            runtime: mint_runtime([Err(TransferError::TooOld)]),
            expected_ledger_calls: 1,
            traps: false,
            expected_reason: Some(FailedMintReason::Expired),
        },
        QuarantineCase {
            name: "the ledger rejects the fee of the mint",
            runtime: mint_runtime([Err(TransferError::BadFee {
                expected_fee: Nat::from(10_u8),
            })]),
            expected_ledger_calls: 1,
            traps: false,
            expected_reason: Some(FailedMintReason::Rejected),
        },
        QuarantineCase {
            name: "the ledger takes the mint for a burn below the minimum",
            runtime: mint_runtime([Err(TransferError::BadBurn {
                min_burn_amount: Nat::from(10_u8),
            })]),
            expected_ledger_calls: 1,
            traps: false,
            expected_reason: Some(FailedMintReason::Rejected),
        },
        QuarantineCase {
            name: "the ledger reports insufficient funds on the minting account",
            runtime: mint_runtime([Err(TransferError::InsufficientFunds {
                balance: Nat::from(0_u8),
            })]),
            expected_ledger_calls: 1,
            traps: false,
            expected_reason: Some(FailedMintReason::Rejected),
        },
        QuarantineCase {
            name: "the callback traps after the ledger minted because mint index is not u64",
            runtime: mint_runtime([Ok(Nat::from(u128::MAX))]),
            expected_ledger_calls: 1,
            traps: true,
            expected_reason: None,
        },
    ];

    let mut expected_attempts = std::collections::BTreeMap::new();
    for QuarantineCase {
        name,
        runtime,
        expected_ledger_calls,
        traps,
        expected_reason,
    } in cases
    {
        setup();
        let pending = credited_pending_mint();

        let outcome = AssertUnwindSafe(process_pending_mints(runtime.clone()))
            .catch_unwind()
            .await;

        assert_eq!(outcome.is_err(), traps, "{name}");
        assert_eq!(
            transfer_args_sent_by(&runtime).len(),
            expected_ledger_calls,
            "{name}"
        );
        assert_eq!(
            deposit_status(pending.deposit_id),
            DepositSolStatus::Quarantined {
                signature: pending.sweep_signature.into()
            },
            "{name}"
        );
        EventsAssert::from_recorded().expect_contains_event_eq(EventType::QuarantinedPendingMint {
            deposit_id: pending.deposit_id,
        });
        if let Some(reason) = expected_reason {
            *expected_attempts.entry(reason).or_insert(0_u64) += 1;
        }
        for reason in FailedMintReason::ALL {
            assert_eq!(
                failed_mint_attempt_count(reason),
                expected_attempts.get(&reason).copied().unwrap_or(0),
                "{name}: {reason:?}"
            );
        }
        assert_eq!(runtime.set_timer_call_count(), 0, "{name}");
    }
}

#[tokio::test]
async fn should_reschedule_until_all_pending_mints_are_processed() {
    const NUM_DEPOSITS: usize = MAX_PENDING_MINTS_PER_ROUND + 1;
    setup();
    credit_sweeps_of_deposits(NUM_DEPOSITS);
    let runtime = mint_runtime(
        (0..MAX_PENDING_MINTS_PER_ROUND as u64).map(|block_index| Ok(block_index.into())),
    );

    process_pending_mints(runtime.clone()).await;

    read_state(|s| {
        assert_eq!(s.deposits().minted().len(), MAX_PENDING_MINTS_PER_ROUND);
        assert_eq!(s.deposits().pending_mints().len(), 1);
    });
    assert_eq!(runtime.set_timer_call_count(), 1);

    let runtime = mint_runtime([Ok((MAX_PENDING_MINTS_PER_ROUND as u64).into())]);

    process_pending_mints(runtime.clone()).await;

    read_state(|s| {
        assert_eq!(s.deposits().minted().len(), NUM_DEPOSITS);
        assert!(s.deposits().pending_mints().is_empty());
    });
    assert_eq!(runtime.set_timer_call_count(), 0);
}

#[tokio::test]
async fn should_not_reschedule_after_a_round_of_transient_failures() {
    const NUM_DEPOSITS: usize = MAX_PENDING_MINTS_PER_ROUND + 1;
    setup();
    credit_sweeps_of_deposits(NUM_DEPOSITS);
    let runtime = mint_runtime(
        (0..MAX_PENDING_MINTS_PER_ROUND).map(|_| Err(TransferError::TemporarilyUnavailable)),
    );

    process_pending_mints(runtime.clone()).await;

    read_state(|s| {
        assert_eq!(s.deposits().pending_mints().len(), NUM_DEPOSITS);
        assert!(s.deposits().minted().is_empty());
    });
    assert_eq!(
        transfer_args_sent_by(&runtime).len(),
        MAX_PENDING_MINTS_PER_ROUND
    );
    assert_eq!(runtime.set_timer_call_count(), 0);
}

fn setup() {
    reset_state();
    reset_events();
    init_state();
    init_schnorr_master_key();
}

fn credited_pending_mint() -> PendingMintFlow {
    DepositFlow::queue(account(1), SWEEPABLE_AMOUNT)
        .sweep(signature(SWEEP_SIGNATURE_INDEX))
        .succeed()
        .credit_at(CREDITED_AT_TIME)
        .single_pending_mint()
}

fn credit_sweeps_of_deposits(num_deposits: usize) -> Vec<PendingMintFlow> {
    let deposits: Vec<_> = (0..num_deposits)
        .map(|index| DepositFlow::queue(account(index + 1), SWEEPABLE_AMOUNT))
        .collect();
    deposits
        .chunks(MAX_DEPOSITS_PER_SWEEP)
        .enumerate()
        .flat_map(|(sweep_index, deposits)| {
            SweepFlow::of(deposits.iter().copied())
                .submit(signature(SWEEP_SIGNATURE_INDEX + sweep_index))
                .succeed()
                .credit()
                .mints
        })
        .collect()
}

fn mint_runtime<I: IntoIterator<Item = MintResult>>(results: I) -> TestCanisterRuntime {
    let mut runtime = TestCanisterRuntime::new().with_increasing_time();
    for result in results {
        runtime = runtime.add_stub_response(result);
    }
    runtime
}

fn deposit_sol_runtime() -> TestCanisterRuntime {
    TestCanisterRuntime::new()
        .with_increasing_time()
        .expecting_charges()
        .add_msg_cycles_available(DEPOSIT_SOL_REQUIRED_CYCLES)
        .add_msg_cycles_refunded(GET_BALANCE_CYCLES / 2)
        .add_stub_response(MultiRpcResult::<Lamport>::Consistent(Ok(
            MINIMUM_DEPOSIT_AMOUNT,
        )))
}

fn expected_transfer_arg(pending: &PendingMintFlow) -> TransferArg {
    TransferArg {
        from_subaccount: None,
        to: pending.account,
        fee: None,
        created_at_time: Some(pending.created_at_time),
        memo: Some(Memo::from(MintMemo::sweep(pending.sweep_signature, pending.deposit_id)).into()),
        amount: NumTokens::from(pending.amount_to_mint),
    }
}

fn transfer_args_sent_by(runtime: &TestCanisterRuntime) -> Vec<TransferArg> {
    runtime
        .sent_update_calls()
        .iter()
        .map(|call| {
            assert_eq!(call.method, "icrc1_transfer");
            call.single_arg()
        })
        .collect()
}

fn deposit_status(deposit_id: DepositSolId) -> DepositSolStatus {
    read_state(|s| s.deposits().status(deposit_id))
}
