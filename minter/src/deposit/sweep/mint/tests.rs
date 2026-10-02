use super::process_pending_mints;
use crate::{
    constants::{GET_BALANCE_CYCLES, LEDGER_DEDUPLICATION_WINDOW, MAX_PENDING_MINTS_PER_ROUND},
    deposit::sweep::{deposit_sol, timer::MAX_DEPOSITS_PER_SWEEP},
    state::{TaskType, event::EventType, mutate_state, read_state, reset_state},
    storage::reset_events,
    test_fixtures::{
        BLOCK_INDEX, DEPOSIT_SOL_REQUIRED_CYCLES, EventsAssert, MINIMUM_DEPOSIT_AMOUNT, account,
        deposit_address,
        flow::deposit::{DepositFlow, PendingMintFlow, SweepFlow},
        init_schnorr_master_key, init_state,
        runtime::{CallResponse, TestCanisterRuntime},
        signature,
    },
};
use candid::Nat;
use cksol_types::{DepositSolId, DepositSolStatus, Memo, MintMemo};
use futures::FutureExt;
use ic_canister_runtime::IcError;
use icrc_ledger_types::icrc1::{
    account::Account,
    transfer::{BlockIndex, NumTokens, TransferArg, TransferError},
};
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
}

#[tokio::test]
async fn should_mint_pending_deposit_and_release_the_account() {
    setup();
    let pending = credited_pending_mint();
    let runtime = mint_runtime([(pending, Ok(BLOCK_INDEX.into()))]);

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
    assert_eq!(runtime.set_timer_call_count(), 0);

    let new_deposit_id = deposit_sol(&deposit_sol_runtime(pending.account), pending.account).await;
    assert_eq!(new_deposit_id, Ok(pending.deposit_id + 1));
}

#[tokio::test]
async fn should_record_duplicate_reply_as_minted() {
    setup();
    let pending = credited_pending_mint();
    let runtime = mint_runtime([(
        pending,
        Err(TransferError::Duplicate {
            duplicate_of: BlockIndex::from(BLOCK_INDEX),
        }),
    )]);

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
        let pending = credited_pending_mint();
        let events_before = EventsAssert::from_recorded();
        let failing_runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .expect_icrc1_transfer(expected_transfer_arg(&pending), failure);

        process_pending_mints(failing_runtime.clone()).await;

        assert_eq!(
            deposit_status(pending.deposit_id),
            DepositSolStatus::Finalized {
                signature: pending.sweep_signature.into()
            },
            "{name}"
        );
        assert_eq!(events_before, EventsAssert::from_recorded(), "{name}");

        let retrying_runtime = mint_runtime([(pending, Ok(BLOCK_INDEX.into()))]);

        process_pending_mints(retrying_runtime.clone()).await;

        assert_eq!(
            deposit_status(pending.deposit_id),
            DepositSolStatus::Minted {
                block_index: BLOCK_INDEX,
                minted_amount: pending.amount_to_mint,
            },
            "{name}"
        );
    }
}

#[tokio::test]
async fn should_quarantine_pending_mint() {
    struct QuarantineCase {
        name: &'static str,
        ledger_response: Option<CallResponse<MintResult>>,
        traps: bool,
    }
    let stale_now = CREDITED_AT_TIME + LEDGER_DEDUPLICATION_WINDOW.as_nanos() as u64 + 1;
    let cases = [
        QuarantineCase {
            name: "the pending mint is older than the deduplication window of the ledger",
            ledger_response: None,
            traps: false,
        },
        QuarantineCase {
            name: "the ledger rejects the mint as too old",
            ledger_response: Some(CallResponse::Reply(Err(TransferError::TooOld))),
            traps: false,
        },
        QuarantineCase {
            name: "the ledger rejects the fee of the mint",
            ledger_response: Some(CallResponse::Reply(Err(TransferError::BadFee {
                expected_fee: Nat::from(10_u8),
            }))),
            traps: false,
        },
        QuarantineCase {
            name: "the ledger takes the mint for a burn below the minimum",
            ledger_response: Some(CallResponse::Reply(Err(TransferError::BadBurn {
                min_burn_amount: Nat::from(10_u8),
            }))),
            traps: false,
        },
        QuarantineCase {
            name: "the ledger reports insufficient funds on the minting account",
            ledger_response: Some(CallResponse::Reply(Err(TransferError::InsufficientFunds {
                balance: Nat::from(0_u8),
            }))),
            traps: false,
        },
        QuarantineCase {
            name: "the callback traps after the ledger minted because mint index is not u64",
            ledger_response: Some(CallResponse::Reply(Ok(Nat::from(u128::MAX)))),
            traps: true,
        },
    ];

    for QuarantineCase {
        name,
        ledger_response,
        traps,
    } in cases
    {
        setup();
        let pending = credited_pending_mint();
        let runtime = match ledger_response {
            None => TestCanisterRuntime::new().add_times([stale_now; 3]),
            Some(response) => TestCanisterRuntime::new()
                .with_increasing_time()
                .expect_icrc1_transfer(expected_transfer_arg(&pending), response),
        };

        let outcome = AssertUnwindSafe(process_pending_mints(runtime.clone()))
            .catch_unwind()
            .await;

        assert_eq!(outcome.is_err(), traps, "{name}");
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
        assert_eq!(runtime.set_timer_call_count(), 0, "{name}");
    }
}

#[tokio::test]
async fn should_reschedule_until_all_pending_mints_are_processed() {
    const NUM_DEPOSITS: usize = MAX_PENDING_MINTS_PER_ROUND + 1;
    setup();
    let pending_mints = credit_sweeps_of_deposits(NUM_DEPOSITS);
    let runtime = mint_runtime(
        pending_mints
            .iter()
            .take(MAX_PENDING_MINTS_PER_ROUND)
            .enumerate()
            .map(|(block_index, pending)| (*pending, Ok((block_index as u64).into()))),
    );

    process_pending_mints(runtime.clone()).await;

    read_state(|s| {
        assert_eq!(s.deposits().minted().len(), MAX_PENDING_MINTS_PER_ROUND);
        assert_eq!(s.deposits().pending_mints().len(), 1);
    });
    assert_eq!(runtime.set_timer_call_count(), 1);

    let runtime = mint_runtime([(
        pending_mints[MAX_PENDING_MINTS_PER_ROUND],
        Ok((MAX_PENDING_MINTS_PER_ROUND as u64).into()),
    )]);

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
    let pending_mints = credit_sweeps_of_deposits(NUM_DEPOSITS);
    let runtime = mint_runtime(
        pending_mints
            .iter()
            .take(MAX_PENDING_MINTS_PER_ROUND)
            .map(|pending| (*pending, Err(TransferError::TemporarilyUnavailable))),
    );

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

fn mint_runtime<I>(mints: I) -> TestCanisterRuntime
where
    I: IntoIterator<Item = (PendingMintFlow, MintResult)>,
{
    let mut runtime = TestCanisterRuntime::new().with_increasing_time();
    for (pending, result) in mints {
        runtime = runtime.expect_icrc1_transfer(expected_transfer_arg(&pending), result);
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

fn deposit_status(deposit_id: DepositSolId) -> DepositSolStatus {
    read_state(|s| s.deposits().status(deposit_id))
}
