use crate::test_fixtures::signer::sign_as_minter;
use crate::{
    constants::{
        FEE_PER_SIGNATURE, MAX_CONCURRENT_RPC_CALLS, MAX_CONCURRENT_SIGNATURES,
        RENT_EXEMPTION_THRESHOLD,
    },
    guard::{TimerGuard, withdrawal_guard},
    sol_transfer::MAX_WITHDRAWALS_PER_NONCE_TX,
    state::{
        MinterTransaction, TaskType,
        event::{Signer, TransactionPurpose},
        read_state,
    },
    test_fixtures::{
        EventsAssert, MINIMUM_WITHDRAWAL_AMOUNT, MINTER_ACCOUNT, MINTER_ADDRESS, NONCE_ACCOUNT,
        WITHDRAWAL_FEE, account, events, init_balance, init_balance_to, init_schnorr_master_key,
        init_state, init_state_with_args, minter_signature, runtime::TestCanisterRuntime,
        signature, valid_init_args,
    },
    withdraw::{
        WITHDRAWAL_PROCESSING_RETRY_DELAY, process_pending_withdrawals, withdraw, withdrawal_status,
    },
};
use assert_matches::assert_matches;
use candid::{Nat, Principal};
use cksol_types::TxFinalizedStatus;
use cksol_types::WithdrawSolStatus;
use cksol_types::{WithdrawSolError, WithdrawSolOk};
use cksol_types_internal::InitArgs;
use ic_canister_runtime::IcError;
use ic_cdk::call::CallRejected;
use ic_cdk_management_canister::SignCallError;
use icrc_ledger_types::{icrc1::account::Account, icrc2::transfer_from::TransferFromError};
use sol_rpc_types::{MultiRpcResult, RpcError};
use solana_signature::Signature;
use std::time::Duration;

const VALID_ADDRESS: &str = "E4MpwNnMWs2XtW5gVrxZvyS7fMq31QD5HvbxmwP45Tz3";

fn test_caller() -> Account {
    Principal::from_slice(&[1_u8; 20]).into()
}

#[tokio::test]
async fn should_return_error_if_calling_ledger_fails() {
    init_state();
    init_schnorr_master_key();

    let runtime = TestCanisterRuntime::new().add_stub_error(IcError::CallPerformFailed);

    let result = withdraw(
        &runtime,
        test_caller(),
        MINIMUM_WITHDRAWAL_AMOUNT,
        VALID_ADDRESS.to_string(),
    )
    .await;

    assert_matches!(
        result,
        Err(WithdrawSolError::TemporarilyUnavailable(e)) => assert!(e.contains("Failed to burn tokens"))
    );
}

#[tokio::test]
async fn should_return_error_if_ledger_unavailable() {
    init_state();
    init_schnorr_master_key();

    let runtime = TestCanisterRuntime::new().add_stub_response(Err::<Nat, TransferFromError>(
        TransferFromError::TemporarilyUnavailable,
    ));

    let result = withdraw(
        &runtime,
        test_caller(),
        MINIMUM_WITHDRAWAL_AMOUNT,
        VALID_ADDRESS.to_string(),
    )
    .await;

    assert_eq!(
        result,
        Err(WithdrawSolError::TemporarilyUnavailable(
            "Ledger is temporarily unavailable".to_string(),
        ))
    );
}

#[tokio::test]
async fn should_return_error_if_insufficient_allowance() {
    init_state();
    init_schnorr_master_key();

    let runtime = TestCanisterRuntime::new().add_stub_response(Err::<Nat, TransferFromError>(
        TransferFromError::InsufficientAllowance {
            allowance: Nat::from(123u64),
        },
    ));

    let result = withdraw(
        &runtime,
        test_caller(),
        MINIMUM_WITHDRAWAL_AMOUNT,
        VALID_ADDRESS.to_string(),
    )
    .await;

    assert_eq!(
        result,
        Err(WithdrawSolError::InsufficientAllowance { allowance: 123u64 })
    );
}

#[tokio::test]
async fn should_return_error_if_insufficient_funds() {
    init_state();
    init_schnorr_master_key();

    let runtime = TestCanisterRuntime::new().add_stub_response(Err::<Nat, TransferFromError>(
        TransferFromError::InsufficientFunds {
            balance: Nat::from(123u64),
        },
    ));

    let result = withdraw(
        &runtime,
        test_caller(),
        MINIMUM_WITHDRAWAL_AMOUNT,
        VALID_ADDRESS.to_string(),
    )
    .await;

    assert_eq!(
        result,
        Err(WithdrawSolError::InsufficientFunds { balance: 123u64 })
    );
}

#[tokio::test]
async fn should_return_temporarily_unavailable_on_generic_error() {
    init_state();
    init_schnorr_master_key();

    let runtime = TestCanisterRuntime::new().add_stub_response(Err::<Nat, TransferFromError>(
        TransferFromError::GenericError {
            error_code: Nat::from(123u64),
            message: "msg".to_string(),
        },
    ));

    let result = withdraw(
        &runtime,
        test_caller(),
        MINIMUM_WITHDRAWAL_AMOUNT,
        VALID_ADDRESS.to_string(),
    )
    .await;

    assert_eq!(
        result,
        Err(WithdrawSolError::TemporarilyUnavailable(
            "Ledger returned a generic error: code 123, message: msg".to_string()
        ))
    );
}

#[tokio::test]
async fn should_return_ok_if_burn_succeeds() {
    init_state();
    init_schnorr_master_key();

    let runtime = TestCanisterRuntime::new()
        .add_stub_response(Ok::<Nat, TransferFromError>(Nat::from(123u64)))
        .with_increasing_time();

    let result = withdraw(
        &runtime,
        test_caller(),
        MINIMUM_WITHDRAWAL_AMOUNT,
        VALID_ADDRESS.to_string(),
    )
    .await;

    assert_eq!(
        result,
        Ok(WithdrawSolOk {
            block_index: 123u64
        })
    );
}

#[tokio::test]
async fn should_return_error_if_address_malformed() {
    init_state();

    let runtime = TestCanisterRuntime::new();

    let result = withdraw(
        &runtime,
        test_caller(),
        MINIMUM_WITHDRAWAL_AMOUNT,
        "not-a-valid-address".to_string(),
    )
    .await;

    assert_matches!(result, Err(WithdrawSolError::MalformedAddress(_)));
}

#[tokio::test]
async fn should_reject_withdrawal_to_invalid_destinations() {
    const SYSTEM_PROGRAM_ID: &str = "11111111111111111111111111111111";
    const CLOCK_SYSVAR_ID: &str = "SysvarC1ock11111111111111111111111111111111";
    init_state();
    init_schnorr_master_key();

    let runtime = TestCanisterRuntime::new();

    let cases = [
        ("the system program", SYSTEM_PROGRAM_ID.to_string()),
        ("a sysvar", CLOCK_SYSVAR_ID.to_string()),
        ("the minter's main address", MINTER_ADDRESS.to_string()),
        ("a nonce account of the pool", NONCE_ACCOUNT.to_string()),
    ];

    for (name, destination) in cases {
        let result = withdraw(
            &runtime,
            test_caller(),
            MINIMUM_WITHDRAWAL_AMOUNT,
            destination,
        )
        .await;

        assert_matches!(
            result,
            Err(WithdrawSolError::InvalidDestination(_)),
            "{name}"
        );
        EventsAssert::assert_no_events_recorded();
    }
}

#[tokio::test]
async fn should_be_temporarily_unavailable_if_nonce_pool_empty() {
    init_state_with_args(InitArgs {
        nonce_accounts: vec![],
        ..valid_init_args()
    });
    init_schnorr_master_key();

    let runtime = TestCanisterRuntime::new();

    let result = withdraw(
        &runtime,
        test_caller(),
        MINIMUM_WITHDRAWAL_AMOUNT,
        VALID_ADDRESS.to_string(),
    )
    .await;

    assert_matches!(
        result,
        Err(WithdrawSolError::TemporarilyUnavailable(e)) => assert!(e.contains("nonce"))
    );
    EventsAssert::assert_no_events_recorded();
}

#[tokio::test]
async fn should_be_temporarily_unavailable_if_minter_public_key_not_cached() {
    init_state();

    let runtime = TestCanisterRuntime::new();

    let result = withdraw(
        &runtime,
        test_caller(),
        MINIMUM_WITHDRAWAL_AMOUNT,
        VALID_ADDRESS.to_string(),
    )
    .await;

    assert_matches!(result, Err(WithdrawSolError::TemporarilyUnavailable(_)));
}

#[tokio::test]
async fn should_return_error_if_amount_too_low() {
    init_state();

    let runtime = TestCanisterRuntime::new();

    let result = withdraw(
        &runtime,
        test_caller(),
        MINIMUM_WITHDRAWAL_AMOUNT - 1,
        VALID_ADDRESS.to_string(),
    )
    .await;

    assert_eq!(
        result,
        Err(WithdrawSolError::ValueTooSmall {
            minimum_withdrawal_amount: MINIMUM_WITHDRAWAL_AMOUNT,
            withdrawal_amount: MINIMUM_WITHDRAWAL_AMOUNT - 1,
        })
    );
}

#[tokio::test]
async fn should_return_error_if_already_processing() {
    init_state();
    init_schnorr_master_key();

    let from = test_caller();
    let _guard = withdrawal_guard(from).unwrap();

    let runtime = TestCanisterRuntime::new();

    let result = withdraw(
        &runtime,
        from,
        MINIMUM_WITHDRAWAL_AMOUNT,
        VALID_ADDRESS.to_string(),
    )
    .await;

    assert_eq!(result, Err(WithdrawSolError::AlreadyProcessing));
}

mod process_pending_withdrawals_tests {
    use super::*;
    use crate::{
        monitor::{MIN_REBROADCAST_AGE, finalize_transactions},
        runtime::CanisterRuntime,
        sol_transfer::build_batch_withdrawal_message,
        state::event::EventType,
        test_fixtures::{
            address, devnet_sweep, durable_nonce,
            events::{
                create_withdrawal_batch_transaction, create_withdrawal_batch_transaction_on,
                submit_withdrawal_batch_transaction,
            },
            nonce_account_info, succeeded_withdrawal_response,
        },
    };

    type GetAccountInfoResult = MultiRpcResult<Option<sol_rpc_types::AccountInfo>>;
    type SendTransactionResult = MultiRpcResult<sol_rpc_types::Signature>;

    #[tokio::test]
    async fn should_do_nothing_if_no_pending_withdrawals() {
        init_state();
        init_schnorr_master_key();

        // We return early, therefore no RPC calls are made
        let runtime = TestCanisterRuntime::new();
        process_pending_withdrawals(runtime).await;

        EventsAssert::assert_no_events_recorded();
    }

    #[tokio::test]
    async fn should_skip_if_already_processing() {
        init_state();

        let _guard = TimerGuard::new(TaskType::WithdrawalProcessing).unwrap();

        // We return early, therefore no RPC calls are made
        let runtime = TestCanisterRuntime::new();
        process_pending_withdrawals(runtime).await;

        EventsAssert::assert_no_events_recorded();
    }

    #[tokio::test]
    async fn should_acquire_and_release_guard() {
        init_state();
        init_schnorr_master_key();

        let runtime = TestCanisterRuntime::new();
        process_pending_withdrawals(runtime).await;

        // Guard should be released, so we can acquire it again
        let _guard = TimerGuard::new(TaskType::WithdrawalProcessing).unwrap();
    }

    #[tokio::test]
    async fn should_skip_withdrawals_when_balance_insufficient() {
        init_state();
        // No init_balance call, so minter balance is 0
        init_schnorr_master_key();

        events::accept_withdrawal(account(1), 0, MINIMUM_WITHDRAWAL_AMOUNT);

        let events_before = EventsAssert::from_recorded();

        let runtime = TestCanisterRuntime::new().with_increasing_time();
        process_pending_withdrawals(runtime).await;

        // No new events should be recorded
        let events_after = EventsAssert::from_recorded();
        assert_eq!(events_before, events_after);

        // Withdrawal should remain pending (not submitted)
        assert_eq!(withdrawal_status(0), WithdrawSolStatus::Pending);
    }

    #[tokio::test]
    async fn should_hold_back_rent_when_withdrawing_exactly_the_minter_balance() {
        init_state();
        init_schnorr_master_key();

        let credited_amount = 12_500_000 - FEE_PER_SIGNATURE;
        init_balance_to(credited_amount);

        let minter_balance = read_state(|s| s.balance());
        assert_eq!(minter_balance, credited_amount);

        let burn_block_index = 3_u64;
        let result = withdraw(
            &TestCanisterRuntime::new()
                .add_stub_response(Ok::<Nat, TransferFromError>(Nat::from(burn_block_index)))
                .with_increasing_time(),
            test_caller(),
            minter_balance + WITHDRAWAL_FEE,
            VALID_ADDRESS.to_string(),
        )
        .await;
        assert_eq!(
            result,
            Ok(WithdrawSolOk {
                block_index: burn_block_index
            })
        );

        let events_before = EventsAssert::from_recorded();

        let runtime = TestCanisterRuntime::new().with_increasing_time();

        process_pending_withdrawals(runtime).await;

        assert_eq!(
            withdrawal_status(burn_block_index),
            WithdrawSolStatus::Pending
        );
        assert_eq!(EventsAssert::from_recorded(), events_before);
        assert_eq!(read_state(|s| s.balance()), minter_balance);
        let _guard = TimerGuard::new(TaskType::WithdrawalProcessing).unwrap();
    }

    #[tokio::test]
    async fn should_process_only_affordable_withdrawals() {
        init_state();
        init_balance_to(12_500_000 + RENT_EXEMPTION_THRESHOLD);
        init_schnorr_master_key();

        // The minter balance is sufficient for the first two withdrawals
        events::accept_withdrawal(account(1), 0, 5_000_000 + WITHDRAWAL_FEE);
        events::accept_withdrawal(account(2), 1, 5_000_000 + WITHDRAWAL_FEE);
        events::accept_withdrawal(account(3), 2, 5_000_000 + WITHDRAWAL_FEE);

        let events_before = EventsAssert::from_recorded();

        let runtime = TestCanisterRuntime::new()
            .add_stub_response(GetAccountInfoResult::Consistent(Ok(Some(
                nonce_account_info(MINTER_ADDRESS, 1),
            ))))
            .add_stub_response(SendTransactionResult::Consistent(Ok(
                minter_signature().into()
            )))
            .add_signer(sign_as_minter())
            .with_increasing_time();

        process_pending_withdrawals(runtime).await;

        // First two withdrawals should be submitted, third should remain pending
        assert_matches!(withdrawal_status(0), WithdrawSolStatus::TxSent { .. });
        assert_matches!(withdrawal_status(1), WithdrawSolStatus::TxSent { .. });
        assert_eq!(withdrawal_status(2), WithdrawSolStatus::Pending);

        // Two new events: the created and the submitted transaction batching both withdrawals
        let events_after = EventsAssert::from_recorded();
        assert_eq!(events_after.len(), events_before.len() + 2);
    }

    #[tokio::test]
    async fn should_record_the_transaction_before_signing_and_submit_it() {
        init_state();
        init_balance();
        init_schnorr_master_key();

        events::accept_withdrawal(account(1), 1, MINIMUM_WITHDRAWAL_AMOUNT);
        let events_before = EventsAssert::from_recorded();

        let runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .add_stub_response(GetAccountInfoResult::Consistent(Ok(Some(
                nonce_account_info(MINTER_ADDRESS, 1),
            ))))
            .add_stub_response(SendTransactionResult::Consistent(Ok(
                minter_signature().into()
            )))
            .add_signer(sign_as_minter());

        process_pending_withdrawals(runtime).await;

        let expected_message = build_batch_withdrawal_message(
            &MINTER_ADDRESS,
            &NONCE_ACCOUNT,
            durable_nonce(1),
            &[(
                solana_address::Address::from([0u8; 32]),
                MINIMUM_WITHDRAWAL_AMOUNT - WITHDRAWAL_FEE,
            )],
        )
        .unwrap();
        let events_after = EventsAssert::from_recorded();
        assert_eq!(events_after.len(), events_before.len() + 2);
        events_after
            .expect_contains_event_eq(EventType::CreatedWithdrawalTransaction {
                burn_indices: vec![1_u64.into()],
                nonce_account: NONCE_ACCOUNT,
                nonce_value: durable_nonce(1),
            })
            .expect_contains_event_eq(EventType::SubmittedTransaction {
                signature: minter_signature(),
                message: expected_message.clone().into(),
                signers: vec![Signer::Minter],
                purpose: TransactionPurpose::Withdrawal {
                    burn_indices: vec![1_u64.into()],
                },
            });

        read_state(|s| {
            let submitted = s.submitted_transactions().get(&minter_signature()).unwrap();
            let MinterTransaction::Withdrawal {
                message,
                nonce_account,
                nonce_value,
                ..
            } = submitted
            else {
                panic!("expected a withdrawal transaction, got {submitted:?}");
            };
            assert_eq!(*message, expected_message.into());
            assert_eq!(*nonce_account, NONCE_ACCOUNT);
            assert_eq!(*nonce_value, durable_nonce(1));
        });
        assert_matches!(withdrawal_status(1), WithdrawSolStatus::TxSent { .. });
    }

    #[tokio::test]
    async fn should_rebroadcast_a_withdrawal_whose_initial_send_failed_until_it_finalizes() {
        init_state();
        devnet_sweep::init_balance();
        events::accept_withdrawal(account(1), 1, MINIMUM_WITHDRAWAL_AMOUNT);
        let minter_address = devnet_sweep::minter_main_address();
        let signature = devnet_sweep::minter_signature_of(
            &build_batch_withdrawal_message(
                &minter_address,
                &NONCE_ACCOUNT,
                durable_nonce(1),
                &[(
                    solana_address::Address::from([0u8; 32]),
                    MINIMUM_WITHDRAWAL_AMOUNT - WITHDRAWAL_FEE,
                )],
            )
            .unwrap(),
        );

        let submission = TestCanisterRuntime::new()
            .with_increasing_time()
            .add_stub_response(GetAccountInfoResult::Consistent(Ok(Some(
                nonce_account_info(minter_address, 1),
            ))))
            .add_stub_response(SendTransactionResult::Consistent(Err(
                RpcError::ValidationError("send failed".to_string()),
            )))
            .add_signer(sign_as_minter().expect([Ok(signature)]));
        process_pending_withdrawals(submission.clone()).await;
        assert_matches!(withdrawal_status(1), WithdrawSolStatus::TxSent { .. });

        let rebroadcast = TestCanisterRuntime::new()
            .with_increasing_time_from(submission.time() + MIN_REBROADCAST_AGE.as_nanos() as u64)
            .add_stub_response(GetAccountInfoResult::Consistent(Ok(Some(
                nonce_account_info(minter_address, 1),
            ))))
            .add_stub_response(SendTransactionResult::Consistent(Ok(signature.into())));
        finalize_transactions(rebroadcast.clone()).await;
        assert_eq!(
            rebroadcast.sent_transactions(),
            submission.sent_transactions()
        );
        assert_matches!(withdrawal_status(1), WithdrawSolStatus::TxSent { .. });

        let finalization = TestCanisterRuntime::new()
            .with_increasing_time()
            .add_stub_response(GetAccountInfoResult::Consistent(Ok(Some(
                nonce_account_info(minter_address, 2),
            ))))
            .add_stub_response(succeeded_withdrawal_response(&signature));
        finalize_transactions(finalization).await;
        assert_eq!(
            withdrawal_status(1),
            WithdrawSolStatus::TxFinalized(TxFinalizedStatus::Success {
                transaction_id: signature.into(),
            })
        );
    }

    #[tokio::test]
    async fn should_skip_the_batch_when_the_nonce_read_fails() {
        init_state();
        init_schnorr_master_key();
        init_balance();
        init_schnorr_master_key();

        events::accept_withdrawal(account(1), 1, MINIMUM_WITHDRAWAL_AMOUNT);
        let events_before = EventsAssert::from_recorded();

        let runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .add_stub_response(GetAccountInfoResult::Consistent(Err(
                RpcError::ValidationError("account unavailable".to_string()),
            )));

        process_pending_withdrawals(runtime).await;

        assert_eq!(EventsAssert::from_recorded(), events_before);
        assert_eq!(withdrawal_status(1), WithdrawSolStatus::Pending);
        assert_nonce_account_free();
    }

    #[tokio::test]
    async fn should_skip_the_batch_when_the_nonce_read_is_stale() {
        init_state();
        init_balance();
        init_schnorr_master_key();

        events::accept_withdrawal(account(1), 1, MINIMUM_WITHDRAWAL_AMOUNT);
        create_withdrawal_batch_transaction(durable_nonce(1), vec![1]);
        submit_withdrawal_batch_transaction(signature(0x50), durable_nonce(1), vec![1]);
        events::succeed_transaction(signature(0x50));

        events::accept_withdrawal(account(2), 2, MINIMUM_WITHDRAWAL_AMOUNT);
        let events_before = EventsAssert::from_recorded();

        let runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .add_stub_response(GetAccountInfoResult::Consistent(Ok(Some(
                nonce_account_info(MINTER_ADDRESS, 1),
            ))));

        process_pending_withdrawals(runtime).await;

        assert_eq!(EventsAssert::from_recorded(), events_before);
        assert_eq!(withdrawal_status(2), WithdrawSolStatus::Pending);
        assert_nonce_account_free();
    }

    #[tokio::test]
    async fn should_re_sign_the_identical_message_after_a_signing_failure() {
        init_state();
        init_balance();
        init_schnorr_master_key();

        events::accept_withdrawal(account(1), 1, MINIMUM_WITHDRAWAL_AMOUNT);
        let expected_message = build_batch_withdrawal_message(
            &MINTER_ADDRESS,
            &NONCE_ACCOUNT,
            durable_nonce(1),
            &[(
                solana_address::Address::from([0u8; 32]),
                MINIMUM_WITHDRAWAL_AMOUNT - WITHDRAWAL_FEE,
            )],
        )
        .unwrap();

        let failing_runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .add_stub_response(GetAccountInfoResult::Consistent(Ok(Some(
                nonce_account_info(MINTER_ADDRESS, 1),
            ))))
            .add_signer(
                sign_as_minter()
                    .of_message(expected_message.serialize())
                    .expect([Err(SignCallError::CallFailed(
                        CallRejected::with_rejection(4, "signing service unavailable".to_string())
                            .into(),
                    ))]),
            );

        process_pending_withdrawals(failing_runtime).await;

        let num_events_after_failure = EventsAssert::from_recorded().len();
        EventsAssert::from_recorded().expect_contains_event_eq(
            EventType::CreatedWithdrawalTransaction {
                burn_indices: vec![1_u64.into()],
                nonce_account: NONCE_ACCOUNT,
                nonce_value: durable_nonce(1),
            },
        );
        assert_eq!(withdrawal_status(1), WithdrawSolStatus::Pending);

        let recovering_runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .add_signer(sign_as_minter().of_message(expected_message.serialize()))
            .add_stub_response(SendTransactionResult::Consistent(Ok(
                minter_signature().into()
            )));

        process_pending_withdrawals(recovering_runtime).await;

        let events_after_recovery = EventsAssert::from_recorded();
        assert_eq!(events_after_recovery.len(), num_events_after_failure + 1);
        events_after_recovery.expect_contains_event_eq(EventType::SubmittedTransaction {
            signature: minter_signature(),
            message: expected_message.into(),
            signers: vec![Signer::Minter],
            purpose: TransactionPurpose::Withdrawal {
                burn_indices: vec![1_u64.into()],
            },
        });
        assert_matches!(withdrawal_status(1), WithdrawSolStatus::TxSent { .. });
    }

    #[tokio::test]
    async fn should_never_share_a_nonce_account_between_concurrent_batches() {
        let second_nonce_account = address(2);
        init_state_with_args(InitArgs {
            nonce_accounts: vec![NONCE_ACCOUNT.to_string(), second_nonce_account.to_string()],
            ..valid_init_args()
        });
        init_balance();
        init_schnorr_master_key();

        let num_requests = MAX_WITHDRAWALS_PER_NONCE_TX + 1;
        for i in 0..num_requests {
            events::accept_withdrawal(account(i), i as u64, MINIMUM_WITHDRAWAL_AMOUNT);
        }

        let runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .add_stub_response(GetAccountInfoResult::Consistent(Ok(Some(
                nonce_account_info(MINTER_ADDRESS, 1),
            ))))
            .add_stub_response(GetAccountInfoResult::Consistent(Ok(Some(
                nonce_account_info(MINTER_ADDRESS, 2),
            ))))
            .add_stub_response(SendTransactionResult::Consistent(Ok(signature(1).into())))
            .add_stub_response(SendTransactionResult::Consistent(Ok(signature(2).into())))
            .add_signer(sign_as_minter().times(2));

        process_pending_withdrawals(runtime).await;

        for i in 0..num_requests {
            assert_matches!(
                withdrawal_status(i as u64),
                WithdrawSolStatus::TxSent { .. }
            );
        }
        read_state(|s| {
            let nonce_accounts: std::collections::BTreeSet<_> = s
                .submitted_transactions()
                .iter()
                .map(|(_, tx)| match tx {
                    MinterTransaction::Withdrawal { nonce_account, .. } => *nonce_account,
                    MinterTransaction::SweepDeposit { .. } => {
                        panic!("expected a withdrawal transaction, got {tx:?}")
                    }
                })
                .collect();
            assert_eq!(
                nonce_accounts,
                [NONCE_ACCOUNT, second_nonce_account].into_iter().collect()
            );
        });
    }

    #[tokio::test]
    async fn should_sign_at_most_the_concurrent_signatures_oldest_first_without_creating_more() {
        let num_bound = MAX_CONCURRENT_SIGNATURES + 1;
        let free_nonce_account = address(num_bound + 1);
        let bound_nonce_account = |burn_index: usize| address(num_bound - burn_index);
        init_state_with_args(InitArgs {
            nonce_accounts: (0..num_bound)
                .map(bound_nonce_account)
                .chain([free_nonce_account])
                .map(|nonce_account| nonce_account.to_string())
                .collect(),
            ..valid_init_args()
        });
        init_balance();
        init_schnorr_master_key();

        for burn_index in 0..num_bound {
            events::accept_withdrawal(
                account(burn_index),
                burn_index as u64,
                MINIMUM_WITHDRAWAL_AMOUNT,
            );
            create_withdrawal_batch_transaction_on(
                bound_nonce_account(burn_index),
                durable_nonce(burn_index),
                vec![burn_index as u64],
            );
        }
        let pending_burn_index = num_bound as u64;
        events::accept_withdrawal(
            account(num_bound),
            pending_burn_index,
            MINIMUM_WITHDRAWAL_AMOUNT,
        );

        let runtime = (0..MAX_CONCURRENT_SIGNATURES)
            .fold(TestCanisterRuntime::new(), |runtime, i| {
                runtime
                    .add_stub_response(SendTransactionResult::Consistent(Ok(signature(i).into())))
            })
            .add_stub_response(GetAccountInfoResult::Consistent(Ok(Some(
                nonce_account_info(MINTER_ADDRESS, num_bound),
            ))))
            .with_increasing_time()
            .add_signer(sign_as_minter().times(MAX_CONCURRENT_SIGNATURES));

        process_pending_withdrawals(runtime).await;

        for burn_index in 0..MAX_CONCURRENT_SIGNATURES {
            assert_matches!(
                withdrawal_status(burn_index as u64),
                WithdrawSolStatus::TxSent { .. }
            );
        }
        assert_eq!(
            withdrawal_status(MAX_CONCURRENT_SIGNATURES as u64),
            WithdrawSolStatus::Pending
        );
        read_state(|s| {
            assert_eq!(
                s.created_withdrawal_txs().keys().collect::<Vec<_>>(),
                vec![&bound_nonce_account(MAX_CONCURRENT_SIGNATURES)]
            );
            assert!(
                s.pending_withdrawal_requests()
                    .contains_key(&pending_burn_index.into())
            );
        });
    }

    #[tokio::test]
    async fn should_cap_the_batches_at_the_free_nonce_accounts() {
        init_state();
        init_balance();
        init_schnorr_master_key();

        let num_requests = MAX_WITHDRAWALS_PER_NONCE_TX + 1;
        for i in 0..num_requests {
            events::accept_withdrawal(account(i), i as u64, MINIMUM_WITHDRAWAL_AMOUNT);
        }

        let runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .add_stub_response(GetAccountInfoResult::Consistent(Ok(Some(
                nonce_account_info(MINTER_ADDRESS, 1),
            ))))
            .add_stub_response(SendTransactionResult::Consistent(Ok(
                minter_signature().into()
            )))
            .add_signer(sign_as_minter());

        process_pending_withdrawals(runtime.clone()).await;

        for i in 0..MAX_WITHDRAWALS_PER_NONCE_TX {
            assert_matches!(
                withdrawal_status(i as u64),
                WithdrawSolStatus::TxSent { .. }
            );
        }
        assert_eq!(
            withdrawal_status(MAX_WITHDRAWALS_PER_NONCE_TX as u64),
            WithdrawSolStatus::Pending
        );
        read_state(|s| assert_eq!(s.submitted_transactions().len(), 1));
        assert_eq!(runtime.set_timer_call_count(), 0);
    }

    #[tokio::test]
    async fn should_retry_later_when_an_affordable_batch_is_left_behind() {
        let second_nonce_account = address(2);
        init_state_with_args(InitArgs {
            nonce_accounts: vec![NONCE_ACCOUNT.to_string(), second_nonce_account.to_string()],
            ..valid_init_args()
        });
        init_balance();
        init_schnorr_master_key();

        let num_requests = MAX_WITHDRAWALS_PER_NONCE_TX + 1;
        for i in 0..num_requests {
            events::accept_withdrawal(account(i), i as u64, MINIMUM_WITHDRAWAL_AMOUNT);
        }

        let runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .add_stub_response(GetAccountInfoResult::Consistent(Ok(Some(
                nonce_account_info(MINTER_ADDRESS, 1),
            ))))
            .add_stub_response(GetAccountInfoResult::Consistent(Err(
                RpcError::ValidationError("account unavailable".to_string()),
            )))
            .add_stub_response(SendTransactionResult::Consistent(Ok(
                minter_signature().into()
            )))
            .add_signer(sign_as_minter());

        process_pending_withdrawals(runtime.clone()).await;

        read_state(|s| assert_eq!(s.submitted_transactions().len(), 1));
        assert_eq!(
            withdrawal_status(MAX_WITHDRAWALS_PER_NONCE_TX as u64),
            WithdrawSolStatus::Pending
        );
        assert_eq!(
            runtime.set_timer_delays(),
            vec![WITHDRAWAL_PROCESSING_RETRY_DELAY]
        );
    }

    #[tokio::test]
    async fn should_not_retry_early_when_the_round_made_no_progress() {
        init_state();
        init_balance();
        init_schnorr_master_key();

        events::accept_withdrawal(account(1), 1, MINIMUM_WITHDRAWAL_AMOUNT);

        let runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .add_stub_response(GetAccountInfoResult::Consistent(Err(
                RpcError::ValidationError("account unavailable".to_string()),
            )));

        process_pending_withdrawals(runtime.clone()).await;

        assert_eq!(withdrawal_status(1), WithdrawSolStatus::Pending);
        assert_eq!(runtime.set_timer_delays(), Vec::<Duration>::new());
    }

    #[tokio::test]
    async fn should_retry_later_when_a_bound_withdrawal_is_left_unsigned() {
        init_state();
        init_balance();
        init_schnorr_master_key();

        events::accept_withdrawal(account(1), 1, MINIMUM_WITHDRAWAL_AMOUNT);

        let runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .add_stub_response(GetAccountInfoResult::Consistent(Ok(Some(
                nonce_account_info(MINTER_ADDRESS, 1),
            ))))
            .add_signer(sign_as_minter().expect([Err(SignCallError::CallFailed(
                CallRejected::with_rejection(4, "signing service unavailable".to_string()).into(),
            ))]));

        process_pending_withdrawals(runtime.clone()).await;

        read_state(|s| assert_eq!(s.created_withdrawal_txs().len(), 1));
        assert_eq!(
            runtime.set_timer_delays(),
            vec![WITHDRAWAL_PROCESSING_RETRY_DELAY]
        );
    }

    #[tokio::test]
    async fn should_not_reschedule_without_a_free_nonce_account_even_with_affordable_batches_left()
    {
        init_state();
        let num_affordable_batches = MAX_CONCURRENT_RPC_CALLS + 1;
        let num_affordable_requests = MAX_CONCURRENT_RPC_CALLS * MAX_WITHDRAWALS_PER_NONCE_TX + 1;
        init_balance_to(
            FEE_PER_SIGNATURE
                + num_affordable_requests as u64 * (MINIMUM_WITHDRAWAL_AMOUNT - WITHDRAWAL_FEE)
                + num_affordable_batches as u64 * FEE_PER_SIGNATURE
                + RENT_EXEMPTION_THRESHOLD,
        );
        init_schnorr_master_key();

        for i in 0..=num_affordable_requests {
            events::accept_withdrawal(account(i), i as u64, MINIMUM_WITHDRAWAL_AMOUNT);
        }

        let runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .add_stub_response(GetAccountInfoResult::Consistent(Ok(Some(
                nonce_account_info(MINTER_ADDRESS, 1),
            ))))
            .add_stub_response(SendTransactionResult::Consistent(Ok(
                minter_signature().into()
            )))
            .add_signer(sign_as_minter());

        process_pending_withdrawals(runtime.clone()).await;

        read_state(|s| assert_eq!(s.submitted_transactions().len(), 1));
        assert_eq!(runtime.set_timer_delays(), Vec::<Duration>::new());
    }

    fn assert_nonce_account_free() {
        read_state(|s| {
            assert_eq!(
                s.nonce_pool().free_accounts().collect::<Vec<_>>(),
                vec![&NONCE_ACCOUNT]
            )
        });
    }
}

mod withdrawal_finalization_tests {
    use super::*;

    fn setup_sent_withdrawal(burn_block_index: u64) -> Signature {
        let tx_signature = signature(burn_block_index as usize + 1);
        events::accept_withdrawal(MINTER_ACCOUNT, burn_block_index, MINIMUM_WITHDRAWAL_AMOUNT);
        events::submit_withdrawal(tx_signature, vec![burn_block_index]);
        tx_signature
    }

    #[test]
    fn should_report_tx_finalized_after_succeeded_transaction() {
        init_state();
        init_balance();
        let tx_signature = setup_sent_withdrawal(1);

        assert_matches!(withdrawal_status(1), WithdrawSolStatus::TxSent { .. });

        events::succeed_transaction(tx_signature);

        assert_eq!(
            withdrawal_status(1),
            WithdrawSolStatus::TxFinalized(TxFinalizedStatus::Success {
                transaction_id: tx_signature.into(),
            })
        );
    }

    #[test]
    fn should_report_tx_failed_after_failed_transaction() {
        init_state();
        init_balance();
        let tx_signature = setup_sent_withdrawal(1);

        assert_matches!(withdrawal_status(1), WithdrawSolStatus::TxSent { .. });

        events::fail_transaction(tx_signature);

        assert_eq!(
            withdrawal_status(1),
            WithdrawSolStatus::TxFinalized(TxFinalizedStatus::Failure {
                transaction_id: tx_signature.into(),
            })
        );
    }
}
