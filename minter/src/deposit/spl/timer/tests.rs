use super::*;
use crate::{
    address::{account_address, associated_token_address},
    state::{MinterTransaction, QueuedSplDeposit},
    test_fixtures::{
        DEFAULT_BLOCK_HEIGHT, account, assert_spl_deposit_metrics, init_balance_to, init_state,
        minter_signature, minter_signature_nth, replayed_recorded_log,
        runtime::TestCanisterRuntime,
        schnorr_master_key,
        signer::{sign_as_minter, sign_for},
    },
};
use candid::Principal;
use cksol_types_internal::{SplTokenRegistration, UpgradeArgs};
use ic_canister_runtime::IcError;
use sol_rpc_types::{
    GetBlockCommitmentLevel, GetBlockParams, GetSlotParams, MultiRpcResult, RpcConfig, RpcError,
    RpcSources, SendTransactionParams,
};

fn test_runtime() -> TestCanisterRuntime {
    TestCanisterRuntime::new().add_times([100; 100])
}
fn record(event: EventType) {
    mutate_state(|state| process_event(state, event, &test_runtime()));
}
fn setup(operating: u64, main: u64) {
    crate::state::reset_state();
    crate::storage::reset_events();
    init_state();
    init_balance_to(main);
    record(EventType::Upgrade(UpgradeArgs {
        add_spl_tokens: Some(
            (2..=3)
                .map(|id| SplTokenRegistration {
                    mint: solana_address::Address::from([id; 32]).to_string(),
                    token_program: spl_token_interface::id().to_string(),
                    decimals: 6,
                    ledger_id: Principal::from_slice(&[id, 1]),
                    index_id: None,
                    symbol: format!("ckTest{id}"),
                    minimum_deposit_amount: 1,
                    deposit_fee: 10,
                    deposits_paused: false,
                    withdrawal_fee_token: None,
                })
                .collect(),
        ),
        ..Default::default()
    }));
    if operating > 0 {
        record(EventType::RequestedOperatingRefill {
            refill_id: 0,
            amount: operating,
            created_at_time: 100,
        });
        record(EventType::CompletedOperatingRefill {
            refill_id: 0,
            burn_block_index: 42,
        });
    }
}
fn queue(id: u64, mint: u8) -> SplSweep {
    let account = account(id as usize + 1);
    let mint = solana_address::Address::from([mint; 32]);
    let deposit = QueuedSplDeposit {
        account,
        mint,
        address: associated_token_address(&account_address(&schnorr_master_key(), &account), &mint),
        balance: 1_000 + id,
        fee: 10,
    };
    record(EventType::QueuedSplDeposit {
        deposit_id: id,
        account,
        mint,
        address: deposit.address,
        balance: deposit.balance,
        fee: deposit.fee,
    });
    read_state(|s| {
        SplSweep::plan(
            id,
            deposit,
            s.supported_spl_token(&mint).unwrap(),
            &schnorr_master_key(),
        )
        .unwrap()
    })
}
fn send_result() -> MultiRpcResult<sol_rpc_types::Signature> {
    MultiRpcResult::Consistent(Ok(minter_signature().into()))
}
fn assert_replay() {
    let replayed = replayed_recorded_log();
    read_state(|state| assert_eq!(state, &replayed));
}

#[tokio::test]
async fn should_leave_unfunded_deposits_queued_without_rpc_or_signing() {
    for (operating, main) in [
        (0, 1_000_000),
        (9_999, 1_000_000),
        (20_000, crate::constants::RENT_EXEMPTION_THRESHOLD + 9_999),
    ] {
        setup(operating, main);
        queue(0, 2);
        let before = read_state(Clone::clone);
        let runtime = test_runtime();
        sweep_queued_spl_deposits(runtime.clone()).await;
        assert!(runtime.sent_update_calls().is_empty());
        read_state(|state| assert_eq!(state, &before));
    }
}

#[test]
fn should_report_queued_deposits_blocked_by_funding_until_it_is_restored() {
    setup(0, 1_000_000);
    assert_spl_deposit_metrics(&["unfunded_queued_spl_deposits 0 "]);
    queue(0, 2);
    queue(1, 3);
    assert_spl_deposit_metrics(&["unfunded_queued_spl_deposits 2 "]);

    for (refill_id, unfunded) in [(0, 1), (1, 0)] {
        record(EventType::RequestedOperatingRefill {
            refill_id,
            amount: SPL_SWEEP_TRANSACTION_FEE,
            created_at_time: 100,
        });
        record(EventType::CompletedOperatingRefill {
            refill_id,
            burn_block_index: 42 + refill_id,
        });
        assert_spl_deposit_metrics(&[&format!("unfunded_queued_spl_deposits {unfunded} ")]);
    }

    // The rent of the main account is not available for sweep fees.
    setup(
        2 * SPL_SWEEP_TRANSACTION_FEE,
        crate::constants::RENT_EXEMPTION_THRESHOLD + SPL_SWEEP_TRANSACTION_FEE,
    );
    queue(0, 2);
    queue(1, 3);
    assert_spl_deposit_metrics(&["unfunded_queued_spl_deposits 1 "]);
}

#[tokio::test]
async fn should_submit_one_transaction_per_deposit_and_replay_reserved_fees() {
    setup(30_000, 2_000_000);
    let first = queue(0, 2);
    let second = queue(1, 3);
    let blockhash = solana_hash::Hash::new_from_array([0x42; 32]);
    let first_message = first.sweep_message(blockhash).serialize();
    let second_message = second.sweep_message(blockhash).serialize();
    let runtime = test_runtime()
        .add_recent_block(Ok(300_000_000))
        .add_signer(sign_as_minter().of_message(first_message.clone()))
        .add_signer(sign_as_minter().of_message(second_message.clone()))
        .add_signer(sign_for(&account(1)).of_message(first_message))
        .add_signer(sign_for(&account(2)).of_message(second_message))
        .add_stub_response(send_result())
        .add_stub_response(send_result());
    sweep_queued_spl_deposits(runtime.clone()).await;
    assert_eq!(runtime.sent_transactions().len(), 2);
    let calls = runtime.sent_update_calls();
    assert_eq!(
        calls
            .iter()
            .map(|call| call.method.as_str())
            .collect::<Vec<_>>(),
        vec!["getSlot", "getBlock", "sendTransaction", "sendTransaction"]
    );
    let (_, _, slot_params): (RpcSources, Option<RpcConfig>, Option<GetSlotParams>) =
        calls[0].args();
    assert_eq!(
        slot_params.and_then(|params| params.commitment),
        Some(CommitmentLevel::Confirmed)
    );
    let (_, _, block_params): (RpcSources, Option<RpcConfig>, GetBlockParams) = calls[1].args();
    assert_eq!(
        block_params.commitment,
        Some(GetBlockCommitmentLevel::Confirmed)
    );
    for call in &calls[2..] {
        let (_, _, params): (RpcSources, Option<RpcConfig>, SendTransactionParams) = call.args();
        assert_eq!(
            params.preflight_commitment,
            Some(CommitmentLevel::Confirmed)
        );
    }
    read_state(|state| {
        assert!(state.spl_deposits().queued().is_empty());
        assert_eq!(state.spl_deposits().swept().len(), 2);
        assert_eq!(state.balance(), 1_980_000);
        assert_eq!(state.operating_balance().balance(), 10_000);
        for (id, mint) in [(0, 2), (1, 3)] {
            let signature = minter_signature_nth(id as usize);
            let submitted = &state.spl_deposits().swept().get(&signature).unwrap();
            assert_eq!(submitted.sweep.deposit_id(), id);
            assert_eq!(submitted.sweep.deposit().amount_to_mint(), 990 + id);
            assert_eq!(submitted.reserved_lamports, 10_000);
            assert_eq!(
                state
                    .spl_deposits()
                    .in_flight_id(&account(id as usize + 1), &[mint; 32].into()),
                Some(id)
            );
            // The shared monitor reads the block height of the sweep from its transaction.
            assert_eq!(
                state.submitted_transactions().get(&signature),
                Some(&MinterTransaction::SweepSplDeposit {
                    block_height: DEFAULT_BLOCK_HEIGHT
                })
            );
        }
        assert_eq!(state.spl_deposits().oldest_in_flight_queued_at(), Some(100));
    });
    assert_replay();
    let no_retry = test_runtime();
    sweep_queued_spl_deposits(no_retry.clone()).await;
    assert!(no_retry.sent_update_calls().is_empty());
}

#[tokio::test]
async fn should_count_a_preflight_rejection_once_for_its_mint_and_reason() {
    use crate::storage::{FailedSplTransferReason, failed_spl_sweep_count};
    setup(30_000, 2_000_000);
    let first = queue(0, 2);
    let second = queue(1, 3);
    let third = queue(2, 3);
    let blockhash = solana_hash::Hash::new_from_array([0x42; 32]);
    let rejection = |cause: &str| -> MultiRpcResult<sol_rpc_types::Signature> {
        MultiRpcResult::Consistent(Err(RpcError::JsonRpcError(sol_rpc_types::JsonRpcError {
            code: -32002,
            message: format!(
                "Transaction simulation failed: Error processing Instruction 0: {cause}"
            ),
        })))
    };
    let mut runtime = test_runtime().add_recent_block(Ok(300_000_000));
    for sweep in [&first, &second, &third] {
        runtime = runtime
            .add_signer(sign_as_minter().of_message(sweep.sweep_message(blockhash).serialize()));
    }
    for (owner, sweep) in [(1, &first), (2, &second), (3, &third)] {
        runtime = runtime.add_signer(
            sign_for(&account(owner)).of_message(sweep.sweep_message(blockhash).serialize()),
        );
    }
    let runtime = runtime
        .add_stub_response(rejection("custom program error: 0x11"))
        .add_stub_response(rejection("invalid account data for instruction"))
        .add_stub_response(send_result());
    sweep_queued_spl_deposits(runtime.clone()).await;
    assert_eq!(runtime.sent_transactions().len(), 3);

    let count = |mint: u8, reason| failed_spl_sweep_count([mint; 32].into(), reason);
    let assert_counts = || {
        for (mint, reason, expected) in [
            (2, FailedSplTransferReason::FrozenAccount, 1),
            (2, FailedSplTransferReason::MissingAccount, 0),
            (2, FailedSplTransferReason::Other, 0),
            (3, FailedSplTransferReason::FrozenAccount, 0),
            (3, FailedSplTransferReason::MissingAccount, 1),
            (3, FailedSplTransferReason::Other, 0),
        ] {
            assert_eq!(count(mint, reason), expected, "{mint} {reason:?}");
        }
    };
    assert_counts();
    let mint = solana_address::Address::from([2; 32]);
    assert_spl_deposit_metrics(&[&format!(
        r#"failed_spl_sweeps{{mint="{mint}",reason="frozen_account"}} 1 "#
    )]);
    // A rejected sweep keeps its intent and is not submitted again, so it is not counted again.
    read_state(|s| assert_eq!(s.spl_deposits().swept().len(), 3));
    let no_retry = test_runtime();
    sweep_queued_spl_deposits(no_retry.clone()).await;
    assert!(no_retry.sent_update_calls().is_empty());
    assert_counts();
}

#[tokio::test]
async fn should_preserve_intent_and_reservation_when_submission_is_ambiguous() {
    setup(10_000, 2_000_000);
    queue(0, 2);
    let runtime = test_runtime()
        .add_recent_block(Ok(300_000_000))
        .add_signer(sign_as_minter())
        .add_signer(sign_for(&account(1)))
        .add_stub_error(IcError::CallPerformFailed);
    sweep_queued_spl_deposits(runtime.clone()).await;
    assert_eq!(runtime.sent_transactions().len(), 1);
    for reason in crate::storage::FailedSplTransferReason::ALL {
        assert_eq!(
            crate::storage::failed_spl_sweep_count([2; 32].into(), reason),
            0
        );
    }
    read_state(|s| {
        assert!(s.spl_deposits().queued().is_empty());
        assert!(s.spl_deposits().swept().get(&minter_signature()).is_some());
        assert_eq!(
            s.spl_deposits()
                .swept()
                .get(&minter_signature())
                .unwrap()
                .reserved_lamports,
            10_000
        );
        assert_eq!(s.operating_balance().balance(), 0);
        assert_eq!(s.balance(), 1_990_000);
    });
    assert_replay();
    let no_retry = test_runtime();
    sweep_queued_spl_deposits(no_retry.clone()).await;
    assert!(no_retry.sent_update_calls().is_empty());
}

#[tokio::test]
async fn should_keep_the_queue_and_balances_when_fetching_the_block_fails() {
    setup(10_000, 2_000_000);
    queue(0, 2);
    let before = read_state(Clone::clone);
    let runtime =
        test_runtime().add_recent_block(Err(RpcError::ValidationError("unavailable".into())));
    sweep_queued_spl_deposits(runtime).await;
    read_state(|s| assert_eq!(s, &before));
}

#[tokio::test]
async fn should_skip_a_signing_failure_without_reserving_its_fee() {
    setup(20_000, 2_000_000);
    queue(0, 2);
    queue(1, 3);
    let runtime = test_runtime()
        .add_recent_block(Ok(300_000_000))
        .add_signer(sign_as_minter().expect([
            Err(ic_cdk_management_canister::SignCallError::CallFailed(
                ic_cdk::call::CallPerformFailed.into(),
            )),
            Ok(minter_signature()),
        ]))
        // try_join_all stops at the first error, before requesting the first owner's signature.
        .add_signer(sign_for(&account(2)))
        .add_stub_response(send_result());
    sweep_queued_spl_deposits(runtime.clone()).await;
    assert_eq!(runtime.sent_transactions().len(), 1);
    read_state(|s| {
        assert!(s.spl_deposits().queued().contains_key(&0));
        assert_eq!(
            s.spl_deposits()
                .swept()
                .get(&minter_signature())
                .unwrap()
                .sweep
                .deposit_id(),
            1
        );
        assert_eq!(s.balance(), 1_990_000);
        assert_eq!(s.operating_balance().balance(), 10_000);
    });
    assert_replay();
}

fn reserve_the_remaining_funds() {
    let sweep = read_state(|state| {
        let deposit = state.spl_deposits().queued()[&1].clone();
        SplSweep::plan(
            1,
            deposit.clone(),
            state.supported_spl_token(&deposit.mint).unwrap(),
            &schnorr_master_key(),
        )
        .unwrap()
    });
    record(EventType::SubmittedTransaction {
        signature: crate::test_fixtures::signature(123),
        message: sweep.sweep_message(solana_hash::Hash::default()).into(),
        signers: sweep.signers().to_vec(),
        purpose: TransactionPurpose::SweepSplDeposit {
            deposit_id: 1,
            block_height: DEFAULT_BLOCK_HEIGHT,
        },
    });
}

#[tokio::test]
async fn should_recheck_available_funds_after_signing_before_recording_or_sending() {
    setup(10_000, 2_000_000);
    queue(0, 2);
    queue(1, 3);
    let runtime = test_runtime()
        .add_recent_block(Ok(300_000_000))
        .add_signer(sign_as_minter().before_reply(reserve_the_remaining_funds))
        .add_signer(sign_for(&account(1)));
    sweep_queued_spl_deposits(runtime.clone()).await;
    assert!(runtime.sent_transactions().is_empty());
    read_state(|s| {
        assert!(s.spl_deposits().queued().contains_key(&0));
        assert_eq!(s.spl_deposits().swept().len(), 1);
        assert!(s.spl_deposits().swept().get(&minter_signature()).is_none());
        assert_eq!(s.operating_balance().balance(), 0);
        assert_eq!(s.balance(), 1_990_000);
    });
    assert_replay();
}

#[tokio::test]
async fn should_record_and_reserve_before_the_submission_call_returns() {
    setup(10_000, 2_000_000);
    queue(0, 2);
    let runtime = test_runtime()
        .add_recent_block(Ok(300_000_000))
        .add_signer(sign_as_minter())
        .add_signer(sign_for(&account(1)))
        .add_stub_response(send_result());
    let sweep = sweep_queued_spl_deposits(runtime.clone());
    tokio::pin!(sweep);
    // The mocked RPC runtime suspends once per call. Inspect the outstanding send.
    while runtime.sent_transactions().is_empty() {
        assert!(futures::poll!(&mut sweep).is_pending());
    }
    read_state(|s| {
        assert!(s.spl_deposits().swept().get(&minter_signature()).is_some());
        assert_eq!(s.operating_balance().balance(), 0);
        assert_eq!(s.balance(), 1_990_000);
        assert_eq!(
            s.spl_deposits().in_flight_id(&account(1), &[2; 32].into()),
            Some(0)
        );
    });
    sweep.await;
    assert_replay();
}

#[tokio::test]
async fn should_bound_a_round_and_continue_past_a_failed_deposit_next_round() {
    setup(1_000_000, 2_000_000);
    for id in 0..=MAX_CONCURRENT_SPL_SWEEPS as u64 {
        queue(id, 2);
    }
    // Every attempted signature fails. The next round must start beyond these entries.
    let first =
        test_runtime()
            .add_recent_block(Ok(300_000_000))
            .add_signer(
                sign_as_minter().expect((0..MAX_CONCURRENT_SPL_SWEEPS).map(|_| {
                    Err(ic_cdk_management_canister::SignCallError::CallFailed(
                        ic_cdk::call::CallPerformFailed.into(),
                    ))
                })),
            );
    sweep_queued_spl_deposits(first.clone()).await;
    assert!(first.sent_transactions().is_empty());
    read_state(|s| {
        assert_eq!(
            s.spl_deposits().queued().len(),
            MAX_CONCURRENT_SPL_SWEEPS + 1
        )
    });
    // The next round tries the untouched ID first, then wraps to retry earlier entries.
    let next_id = MAX_CONCURRENT_SPL_SWEEPS as u64;
    let second = test_runtime()
        .add_recent_block(Ok(300_000_000))
        .add_signer(
            sign_as_minter().expect(std::iter::once(Ok(minter_signature())).chain(
                (1..MAX_CONCURRENT_SPL_SWEEPS).map(|_| {
                    Err(ic_cdk_management_canister::SignCallError::CallFailed(
                        ic_cdk::call::CallPerformFailed.into(),
                    ))
                }),
            )),
        )
        .add_signer(sign_for(&account(next_id as usize + 1)))
        .add_stub_response(send_result());
    sweep_queued_spl_deposits(second.clone()).await;
    assert_eq!(second.sent_transactions().len(), 1);
    read_state(|s| {
        assert_eq!(
            s.spl_deposits()
                .swept()
                .get(&minter_signature())
                .unwrap()
                .sweep
                .deposit_id(),
            next_id
        );
        assert_eq!(s.spl_deposits().queued().len(), MAX_CONCURRENT_SPL_SWEEPS);
        assert_eq!(s.operating_balance().balance(), 990_000);
    });
    assert_replay();
}

#[tokio::test]
async fn should_stop_at_either_budget_without_spending_main_account_rent() {
    let rent = crate::constants::RENT_EXEMPTION_THRESHOLD;
    for (operating, main, expected_operating, expected_main) in [
        (10_000, 2_000_000, 0, 1_990_000),
        (30_000, rent + 10_000, 20_000, rent),
    ] {
        setup(operating, main);
        queue(0, 2);
        queue(1, 3);
        let runtime = test_runtime()
            .add_recent_block(Ok(300_000_000))
            .add_signer(sign_as_minter())
            .add_signer(sign_for(&account(1)))
            .add_stub_response(send_result());
        sweep_queued_spl_deposits(runtime.clone()).await;
        assert_eq!(runtime.sent_transactions().len(), 1);
        read_state(|s| {
            assert_eq!(
                s.spl_deposits()
                    .queued()
                    .keys()
                    .copied()
                    .collect::<Vec<_>>(),
                vec![1]
            );
            assert_eq!(s.spl_deposits().swept().len(), 1);
            assert_eq!(s.operating_balance().balance(), expected_operating);
            assert_eq!(s.balance(), expected_main);
        });
        assert_replay();
    }
}

#[tokio::test]
async fn should_not_overlap_spl_sweep_rounds() {
    setup(10_000, 2_000_000);
    queue(0, 2);
    let runtime = test_runtime()
        .add_recent_block(Ok(300_000_000))
        .add_signer(sign_as_minter())
        .add_signer(sign_for(&account(1)))
        .add_stub_response(send_result());
    let concurrent = test_runtime();
    tokio::join!(
        sweep_queued_spl_deposits(runtime.clone()),
        sweep_queued_spl_deposits(concurrent.clone()),
    );
    assert_eq!(runtime.sent_transactions().len(), 1);
    assert!(concurrent.sent_update_calls().is_empty());
    assert_replay();
}

#[tokio::test]
async fn should_start_a_bounded_round_without_waiting_for_each_submission() {
    setup(100_000, 2_000_000);
    for id in 0..=MAX_CONCURRENT_SPL_SWEEPS as u64 {
        queue(id, 2);
    }
    let mut runtime = test_runtime()
        .add_recent_block(Ok(300_000_000))
        .add_signer(sign_as_minter().times(MAX_CONCURRENT_SPL_SWEEPS));
    for id in 0..MAX_CONCURRENT_SPL_SWEEPS {
        runtime = runtime
            .add_signer(sign_for(&account(id + 1)))
            .add_stub_response(send_result());
    }
    let sweep = sweep_queued_spl_deposits(runtime.clone());
    tokio::pin!(sweep);
    // getSlot and getBlock each suspend once. The third poll starts every sweep;
    // each sendTransaction then suspends before its reply, keeping them all outstanding.
    for _ in 0..3 {
        assert!(futures::poll!(&mut sweep).is_pending());
    }
    assert_eq!(runtime.sent_transactions().len(), 5);
    read_state(|s| {
        assert_eq!(s.spl_deposits().swept().len(), 5);
        assert_eq!(
            s.spl_deposits()
                .queued()
                .keys()
                .copied()
                .collect::<Vec<_>>(),
            vec![5]
        );
        assert_eq!(s.operating_balance().balance(), 50_000);
        assert_eq!(s.balance(), 1_950_000);
    });
    sweep.await;
    assert_replay();
}
