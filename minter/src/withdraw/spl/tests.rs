use super::*;
use crate::{
    constants::{CLASSIC_TOKEN_ACCOUNT_RENT_EXEMPTION, LEDGER_DEDUPLICATION_WINDOW},
    state::{CompletedSplCkSolRefund, PendingSplWithdrawalRequest, reset_state},
    storage::reset_events,
    test_fixtures::{
        EventsAssert, MINTER_ADDRESS, NONCE_ACCOUNT, account, init_schnorr_master_key, init_state,
        init_state_with_args, ledger_canister_id, replayed_recorded_log,
        runtime::TestCanisterRuntime, valid_init_args,
    },
};
use candid::{Nat, Principal};
use cksol_types::SplRefundKind;
use cksol_types_internal::{SplTokenRegistration, SplTokenUpdate, UpgradeArgs};
use ic_canister_runtime::IcError;
use icrc_ledger_types::{icrc1::transfer::TransferError, icrc2::transfer_from::TransferFromArgs};

const FEE: Lamport = 1_000_000;
const RENT_RESERVE: Lamport = CLASSIC_TOKEN_ACCOUNT_RENT_EXEMPTION;
const CKSOL_BURN_AMOUNT: Lamport = FEE + RENT_RESERVE;
const LEDGER_FEE: Lamport = 10_000;
const DESTINATION: &str = "E4MpwNnMWs2XtW5gVrxZvyS7fMq31QD5HvbxmwP45Tz3";

fn mint() -> Address {
    [3; 32].into()
}

fn record(event: EventType) {
    mutate_state(|state| {
        process_event(state, event, &TestCanisterRuntime::new().add_times([1; 10]))
    });
}

/// Registers one token with the given withdrawal fee, in the token's smallest units.
fn register_token(withdrawal_fee_token: Option<u64>) -> SupportedSplToken {
    record(EventType::Upgrade(UpgradeArgs {
        add_spl_tokens: Some(vec![SplTokenRegistration {
            mint: mint().to_string(),
            token_program: spl_token_interface::id().to_string(),
            decimals: 6,
            ledger_id: Principal::from_slice(&[3, 1]),
            index_id: None,
            symbol: "ckTest".to_string(),
            minimum_deposit_amount: 1_000,
            deposit_fee: 10,
            deposits_paused: false,
            withdrawal_fee_token,
        }]),
        ..Default::default()
    }));
    read_state(|state| state.supported_spl_token(&mint()).cloned().unwrap())
}

fn setup() -> SupportedSplToken {
    reset_state();
    reset_events();
    init_state();
    init_schnorr_master_key();
    register_token(None)
}

fn withdrawal_terms(token: &SupportedSplToken) -> SplWithdrawalRequestTerms {
    SplWithdrawalRequestTerms {
        account: Account {
            subaccount: Some([7; 32]),
            ..account(1)
        },
        mint: token.mint,
        ledger_id: token.ledger_id,
        address: DESTINATION.parse().unwrap(),
        amount: u64::MAX,
        fee: FEE,
        rent_reserve: RENT_RESERVE,
        token_fee: token.withdrawal_fee_token,
    }
}

fn assert_nothing_recorded(events_before: EventsAssert) {
    read_state(|state| {
        assert!(state.pending_spl_withdrawal_requests().is_empty());
        assert!(state.pending_spl_cksol_refunds().is_empty());
    });
    assert_eq!(EventsAssert::from_recorded(), events_before);
}

async fn withdraw_request(
    runtime: &TestCanisterRuntime,
    request: &SplWithdrawalRequestTerms,
) -> Result<WithdrawSplOk, WithdrawSplError> {
    withdraw(
        runtime,
        request.account,
        request.mint,
        request.amount,
        &request.address.to_string(),
    )
    .await
}

fn burned(block_index: u64) -> Result<Nat, TransferFromError> {
    Ok(block_index.into())
}

fn reimbursement_runtime<I: IntoIterator<Item = Result<Nat, TransferError>>>(
    replies: I,
) -> TestCanisterRuntime {
    // One time for the whole round, as within one message on the Internet Computer.
    let mut runtime = TestCanisterRuntime::new().add_times([1_000; 30]);
    for reply in replies {
        runtime = runtime.add_stub_response(reply);
    }
    runtime
}

fn queue_fee_reimbursement(
    token: &SupportedSplToken,
    cksol_burn_block_index: u64,
) -> PendingSplCkSolRefund {
    schedule_withdrawal_fee_reimbursement(
        &TestCanisterRuntime::new().add_times([999; 10]),
        cksol_burn_block_index.into(),
        &withdrawal_terms(token),
        CKSOL_BURN_AMOUNT,
        &SplBurnError::TransferFrom(TransferFromError::TemporarilyUnavailable),
        LEDGER_FEE,
    );
    read_state(|state| state.pending_spl_cksol_refunds()[&cksol_burn_block_index].clone())
}

fn queued_fee_reimbursement() -> PendingSplCkSolRefund {
    let token = setup();
    queue_fee_reimbursement(&token, 42)
}

fn assert_fee_reimbursed(cksol_burn_block_index: u64, mint_block_index: u64) {
    read_state(|state| {
        assert!(
            !state
                .pending_spl_cksol_refunds()
                .contains_key(&cksol_burn_block_index)
        );
        assert_eq!(
            state.completed_spl_cksol_refunds()[&cksol_burn_block_index].mint_block_index,
            mint_block_index.into()
        );
    });
    EventsAssert::from_recorded().expect_contains_event_eq(EventType::ReimbursedSplCkSol {
        cksol_burn_block_index: cksol_burn_block_index.into(),
        kind: SplRefundKind::FailedTokenBurn,
        mint_block_index: mint_block_index.into(),
    });
}

fn assert_fee_quarantined(cksol_burn_block_index: u64, pending: PendingSplCkSolRefund) {
    read_state(|state| {
        assert!(
            !state
                .pending_spl_cksol_refunds()
                .contains_key(&cksol_burn_block_index)
        );
        assert_eq!(
            state.quarantined_spl_cksol_refunds()[&cksol_burn_block_index],
            pending
        );
    });
    EventsAssert::from_recorded().expect_contains_event_eq(EventType::QuarantinedSplCkSolRefund {
        cksol_burn_block_index: cksol_burn_block_index.into(),
        kind: SplRefundKind::FailedTokenBurn,
    });
}

/// Replays the recorded log through its stored bytes and its public Candid form.
fn assert_replay() {
    use crate::state::event::Event;
    use ic_stable_structures::Storable;
    for event in crate::storage::with_event_iter(|events| events.collect::<Vec<Event>>()) {
        let decoded = Event::from_bytes(std::borrow::Cow::Owned(event.to_bytes().into_owned()));
        let wire = candid::encode_one(cksol_types_internal::event::Event::from(event)).unwrap();
        let public: cksol_types_internal::event::Event = candid::decode_one(&wire).unwrap();
        assert_eq!(Event::try_from(public).unwrap(), decoded);
    }
    let replayed = replayed_recorded_log();
    read_state(|state| assert_eq!(state, &replayed));
}

mod validation {
    use super::*;

    #[test]
    fn should_require_only_an_amount_above_the_token_fee() {
        setup();
        assert_eq!(
            validate_token(mint(), 0),
            Err(WithdrawSplError::ValueTooSmall {
                minimum_withdrawal_amount: 1,
                withdrawal_amount: 0,
            })
        );
        // No minimum applies: one unit is below the deposit minimum of the token.
        assert_eq!(validate_token(mint(), 1).unwrap().withdrawal_fee_token, 0);

        record(EventType::Upgrade(UpgradeArgs {
            update_spl_tokens: Some(vec![SplTokenUpdate {
                mint: mint().to_string(),
                minimum_deposit_amount: None,
                deposit_fee: None,
                deposits_paused: None,
                withdrawal_fee_token: Some(5),
            }]),
            ..Default::default()
        }));
        for amount in [0, 5] {
            assert_eq!(
                validate_token(mint(), amount),
                Err(WithdrawSplError::ValueTooSmall {
                    minimum_withdrawal_amount: 6,
                    withdrawal_amount: amount,
                })
            );
        }
        assert_eq!(validate_token(mint(), 6).unwrap().withdrawal_fee_token, 5);
    }

    #[test]
    fn should_reject_an_unregistered_mint() {
        setup();
        let other: Address = [4; 32].into();
        assert_eq!(
            validate_token(other, 1),
            Err(WithdrawSplError::TokenNotSupported { mint: other.into() })
        );
    }

    #[test]
    fn should_accept_a_withdrawal_of_a_paused_token() {
        setup();
        record(EventType::Upgrade(UpgradeArgs {
            update_spl_tokens: Some(vec![SplTokenUpdate {
                mint: mint().to_string(),
                minimum_deposit_amount: None,
                deposit_fee: None,
                deposits_paused: Some(true),
                withdrawal_fee_token: None,
            }]),
            ..Default::default()
        }));
        // The pause stops new deposits only.
        assert!(validate_token(mint(), 1).is_ok());
    }

    #[test]
    fn should_reject_a_malformed_destination() {
        let token = setup();
        for address in [
            "",
            "not-an-address",
            "0x0000000000000000000000000000000000000000",
        ] {
            assert!(matches!(
                validate_destination(&token, address),
                Err(WithdrawSplError::MalformedAddress(_))
            ));
        }
    }

    #[test]
    fn should_reject_reserved_minter_nonce_treasury_and_off_curve_destinations() {
        let token = setup();
        let treasury = associated_token_address(&MINTER_ADDRESS, &token.mint);
        // The token account of another wallet is a program-derived address.
        let other_token_account =
            associated_token_address(&DESTINATION.parse().unwrap(), &token.mint);
        for (destination, reason) in [
            (
                solana_sdk_ids::system_program::ID.to_string(),
                "reserved by the Solana runtime",
            ),
            (MINTER_ADDRESS.to_string(), "minter's main address"),
            (NONCE_ACCOUNT.to_string(), "durable nonce account"),
            (treasury.to_string(), "treasury token account"),
            (other_token_account.to_string(), "off the ed25519 curve"),
        ] {
            let error = validate_destination(&token, &destination).unwrap_err();
            assert!(
                matches!(&error, WithdrawSplError::InvalidDestination(message) if message.contains(reason)),
                "{destination}: {error:?}"
            );
        }
    }

    #[test]
    fn should_return_the_parsed_destination_wallet() {
        let token = setup();
        assert_eq!(
            validate_destination(&token, DESTINATION),
            Ok(DESTINATION.parse().unwrap())
        );
    }

    #[test]
    fn should_be_temporarily_unavailable_without_the_minter_public_key() {
        reset_state();
        reset_events();
        init_state();
        let token = register_token(None);
        assert!(matches!(
            validate_destination(&token, DESTINATION),
            Err(WithdrawSplError::TemporarilyUnavailable(_))
        ));
    }

    #[tokio::test]
    async fn should_reject_an_empty_nonce_pool_before_either_burn() {
        reset_state();
        reset_events();
        init_state_with_args(valid_init_args());
        init_schnorr_master_key();
        let token = register_token(None);
        let events_before = EventsAssert::from_recorded();
        let runtime = TestCanisterRuntime::new();

        let result = withdraw_request(&runtime, &withdrawal_terms(&token)).await;

        assert!(matches!(
            result,
            Err(WithdrawSplError::TemporarilyUnavailable(message)) if message.contains("nonce account pool is empty")
        ));
        assert!(runtime.sent_update_calls().is_empty());
        assert_nothing_recorded(events_before);
    }

    #[tokio::test]
    async fn should_reject_invalid_withdrawals_before_either_burn() {
        let token = setup();
        let events_before = EventsAssert::from_recorded();
        let valid = withdrawal_terms(&token);
        let runtime = TestCanisterRuntime::new();

        let zero_amount = SplWithdrawalRequestTerms {
            amount: 0,
            ..valid.clone()
        };
        assert!(matches!(
            withdraw_request(&runtime, &zero_amount).await,
            Err(WithdrawSplError::ValueTooSmall { .. })
        ));
        let unknown_mint = SplWithdrawalRequestTerms {
            mint: [9; 32].into(),
            ..valid.clone()
        };
        assert!(matches!(
            withdraw_request(&runtime, &unknown_mint).await,
            Err(WithdrawSplError::TokenNotSupported { .. })
        ));
        let to_minter = SplWithdrawalRequestTerms {
            address: MINTER_ADDRESS,
            ..valid.clone()
        };
        assert!(matches!(
            withdraw_request(&runtime, &to_minter).await,
            Err(WithdrawSplError::InvalidDestination(_))
        ));
        assert!(matches!(
            withdraw(
                &runtime,
                valid.account,
                valid.mint,
                valid.amount,
                "malformed"
            )
            .await,
            Err(WithdrawSplError::MalformedAddress(_))
        ));

        assert!(runtime.sent_update_calls().is_empty());
        assert_nothing_recorded(events_before);
        // Each rejected request released the account guard.
        assert!(withdrawal_guard(valid.account).is_ok());
    }
}

mod fee_burn {
    use super::*;

    #[tokio::test]
    async fn should_burn_the_fee_and_rent_reserve_together_without_recording_an_event() {
        let token = setup();
        let request = withdrawal_terms(&token);
        let _guard = withdrawal_guard(request.account).unwrap();
        let runtime = TestCanisterRuntime::new()
            .add_stub_response(burned(42))
            .add_times([123; 10]);
        let events_before = EventsAssert::from_recorded();

        assert_eq!(request.cksol_burn_amount(), Some(CKSOL_BURN_AMOUNT));
        assert_eq!(
            burn_withdrawal_fee(&runtime, &request, CKSOL_BURN_AMOUNT).await,
            Ok(42.into())
        );

        let calls = runtime.sent_update_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].canister_id, ledger_canister_id());
        assert_eq!(calls[0].method, "icrc2_transfer_from");
        assert_eq!(
            calls[0].single_arg::<TransferFromArgs>(),
            TransferFromArgs {
                spender_subaccount: None,
                from: request.account,
                to: runtime.canister_self().into(),
                amount: CKSOL_BURN_AMOUNT.into(),
                fee: None,
                memo: Some(
                    Memo::spl_withdrawal_fee(request.mint, request.amount, request.address).into()
                ),
                created_at_time: None,
            }
        );
        assert_nothing_recorded(events_before);
        // The caller retains the guard for the next token burn.
        assert_eq!(
            withdrawal_guard(request.account).err(),
            Some(GuardError::AlreadyProcessing)
        );
    }

    #[tokio::test]
    async fn should_return_the_ledger_rejection_of_the_fee_burn_without_a_token_burn() {
        let token = setup();
        let request = withdrawal_terms(&token);
        let events_before = EventsAssert::from_recorded();
        let runtime = TestCanisterRuntime::new().add_stub_response(Err::<Nat, _>(
            TransferFromError::InsufficientAllowance {
                allowance: 5_u64.into(),
            },
        ));

        assert_eq!(
            withdraw_request(&runtime, &request).await,
            Err(WithdrawSplError::CkSolLedgerError {
                error: LedgerError::InsufficientAllowance {
                    allowance: 5_u64.into(),
                    failed_burn_amount: CKSOL_BURN_AMOUNT.into(),
                    token_symbol: "ckSOL".to_string(),
                    ledger_id: ledger_canister_id(),
                }
            })
        );

        assert_eq!(runtime.sent_update_calls().len(), 1);
        assert_nothing_recorded(events_before);
        assert!(withdrawal_guard(request.account).is_ok());
    }

    #[tokio::test]
    async fn should_return_a_failed_fee_burn_call_as_temporarily_unavailable() {
        let token = setup();
        let request = withdrawal_terms(&token);
        let events_before = EventsAssert::from_recorded();
        let runtime = TestCanisterRuntime::new().add_stub_error(IcError::CallPerformFailed);

        assert!(matches!(
            withdraw_request(&runtime, &request).await,
            Err(WithdrawSplError::CkSolLedgerError {
                error: LedgerError::TemporarilyUnavailable(message)
            }) if message.starts_with("ckSOL ledger: ")
        ));

        assert_eq!(runtime.sent_update_calls().len(), 1);
        assert_nothing_recorded(events_before);
    }
}

mod token_burn {
    use super::*;

    #[tokio::test]
    async fn should_burn_both_ledgers_then_accept_the_spl_withdrawal() {
        let token = setup();
        let request = withdrawal_terms(&token);
        let events_before = EventsAssert::from_recorded();
        let runtime = TestCanisterRuntime::new()
            .add_stub_response(burned(42))
            .add_stub_response(burned(7))
            .add_times([123; 10]);

        assert_eq!(
            withdraw_request(&runtime, &request).await,
            Ok(WithdrawSplOk {
                cksol_burn_block_index: 42,
                ckspl_burn_block_index: 7,
            })
        );

        let calls = runtime.sent_update_calls();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].canister_id, ledger_canister_id());
        assert_eq!(
            calls[0].single_arg::<TransferFromArgs>().amount,
            Nat::from(CKSOL_BURN_AMOUNT)
        );
        assert_eq!(calls[1].canister_id, token.ledger_id);
        assert_eq!(calls[1].method, "icrc2_transfer_from");
        assert_eq!(
            calls[1].single_arg::<TransferFromArgs>(),
            TransferFromArgs {
                spender_subaccount: None,
                from: request.account,
                to: runtime.canister_self().into(),
                amount: request.amount.into(),
                fee: None,
                memo: Some(Memo::spl_convert(42, request.address).into()),
                created_at_time: None,
            }
        );
        let accepted = SplWithdrawalRequest {
            terms: request.clone(),
            cksol_burn_block_index: 42.into(),
            ckspl_burn_block_index: 7.into(),
        };
        assert_eq!(EventsAssert::from_recorded().len(), events_before.len() + 1);
        EventsAssert::from_recorded()
            .expect_contains_event_eq(EventType::AcceptedSplWithdrawalRequest(accepted.clone()));
        read_state(|state| {
            assert!(state.pending_spl_cksol_refunds().is_empty());
            assert_eq!(
                state.pending_spl_withdrawal_requests(),
                &[(
                    42.into(),
                    PendingSplWithdrawalRequest {
                        request: accepted,
                        created_at: 123,
                    }
                )]
                .into()
            );
        });
        assert_eq!(withdrawal_status(42), WithdrawSplStatus::Pending);
        assert_eq!(withdrawal_status(7), WithdrawSplStatus::NotFound);
        assert!(withdrawal_guard(request.account).is_ok());
        assert_replay();
    }

    #[tokio::test]
    async fn should_capture_the_fees_at_the_request_and_keep_them_after_a_change() {
        let token = setup();
        record(EventType::Upgrade(UpgradeArgs {
            update_spl_tokens: Some(vec![SplTokenUpdate {
                mint: mint().to_string(),
                minimum_deposit_amount: None,
                deposit_fee: None,
                deposits_paused: None,
                withdrawal_fee_token: Some(25),
            }]),
            spl_withdrawal_fee: Some(1_500_000),
            ..Default::default()
        }));
        let request = SplWithdrawalRequestTerms {
            amount: 1_000,
            ..withdrawal_terms(&token)
        };
        let runtime = TestCanisterRuntime::new()
            .add_stub_response(burned(42))
            .add_stub_response(burned(7))
            .add_times([123; 10]);

        withdraw_request(&runtime, &request).await.unwrap();

        // The ckSOL burn uses the new fee; the token burn is the full amount.
        let calls = runtime.sent_update_calls();
        assert_eq!(
            calls[0].single_arg::<TransferFromArgs>().amount,
            Nat::from(1_500_000 + RENT_RESERVE)
        );
        assert_eq!(
            calls[1].single_arg::<TransferFromArgs>().amount,
            Nat::from(1_000_u64)
        );
        record(EventType::Upgrade(UpgradeArgs {
            update_spl_tokens: Some(vec![SplTokenUpdate {
                mint: mint().to_string(),
                minimum_deposit_amount: None,
                deposit_fee: None,
                deposits_paused: None,
                withdrawal_fee_token: Some(0),
            }]),
            spl_withdrawal_fee: Some(2_000_000),
            ..Default::default()
        }));
        read_state(|state| {
            let terms = &state.pending_spl_withdrawal_requests()[&42.into()]
                .request
                .terms;
            assert_eq!(terms.fee, 1_500_000);
            assert_eq!(terms.rent_reserve, RENT_RESERVE);
            assert_eq!(terms.token_fee, 25);
            assert_eq!(terms.amount_to_transfer(), Some(975));
        });
        assert_replay();
    }

    #[tokio::test]
    #[should_panic(expected = "BUG: unexpected ckSPL burn error")]
    async fn should_reject_an_unexpected_duplicate_token_burn_reply() {
        let token = setup();
        let request = withdrawal_terms(&token);
        let runtime = TestCanisterRuntime::new().add_stub_response(Err::<Nat, _>(
            TransferFromError::Duplicate {
                duplicate_of: 7_u64.into(),
            },
        ));
        let _ = burn_withdrawal_amount(&runtime, 42.into(), &request).await;
    }
}

mod fee_reimbursements {
    use super::*;

    #[tokio::test]
    async fn should_queue_the_refund_of_the_fee_and_rent_reserve_like_ckerc20() {
        let token = setup();
        let request = SplWithdrawalRequestTerms {
            amount: 1,
            ..withdrawal_terms(&token)
        };
        let ledger_fee = CKSOL_LEDGER_TRANSACTION_FEE;
        let rejections = [
            (
                TransferFromError::InsufficientFunds {
                    balance: 0_u64.into(),
                },
                CKSOL_BURN_AMOUNT - ledger_fee,
            ),
            (
                TransferFromError::InsufficientAllowance {
                    allowance: 0_u64.into(),
                },
                CKSOL_BURN_AMOUNT - ledger_fee,
            ),
            (
                TransferFromError::BadBurn {
                    min_burn_amount: 2_u64.into(),
                },
                CKSOL_BURN_AMOUNT - ledger_fee,
            ),
            (TransferFromError::TemporarilyUnavailable, CKSOL_BURN_AMOUNT),
            (
                TransferFromError::GenericError {
                    error_code: 1_u64.into(),
                    message: "ledger error".to_string(),
                },
                CKSOL_BURN_AMOUNT,
            ),
        ];
        // A failed call is refunded in full, as a ledger that is unavailable.
        let token_burn_replies = rejections
            .into_iter()
            .map(|(error, refund)| (Some(error), refund))
            .chain([(None, CKSOL_BURN_AMOUNT)]);

        for (index, (rejection, reimbursed_amount)) in token_burn_replies.enumerate() {
            let burn_index = 42 + index as u64;
            let runtime = TestCanisterRuntime::new()
                .add_stub_response(burned(burn_index))
                .add_times([999; 10]);
            let runtime = match rejection {
                Some(error) => runtime.add_stub_response(Err::<Nat, _>(error)),
                None => runtime.add_stub_error(IcError::CallPerformFailed),
            };

            let result = withdraw_request(&runtime, &request).await;

            assert!(
                matches!(
                    result,
                    Err(WithdrawSplError::CkSplLedgerError { cksol_burn_block_index, .. })
                        if cksol_burn_block_index == burn_index
                ),
                "{result:?}"
            );
            let calls = runtime.sent_update_calls();
            assert_eq!(calls.len(), 2);
            assert_eq!(calls[1].canister_id, token.ledger_id);
            read_state(|state| {
                assert!(state.pending_spl_withdrawal_requests().is_empty());
                assert_eq!(
                    state.pending_spl_cksol_refunds()[&burn_index],
                    PendingSplCkSolRefund {
                        kind: SplRefundKind::FailedTokenBurn,
                        to: request.account,
                        amount: reimbursed_amount,
                        created_at_time: 999,
                    }
                );
            });
            // The outcome and its refund are recorded together, in this order.
            let events = crate::storage::with_event_iter(|events| events.collect::<Vec<_>>());
            assert_eq!(
                events[events.len() - 2..]
                    .iter()
                    .map(|event| event.payload.clone())
                    .collect::<Vec<_>>(),
                [
                    EventType::FailedSplWithdrawalRequest {
                        cksol_burn_block_index: burn_index.into(),
                        to: request.account,
                        reimbursed_amount,
                    },
                    EventType::QueuedSplCkSolRefund {
                        cksol_burn_block_index: burn_index.into(),
                        kind: SplRefundKind::FailedTokenBurn,
                        to: request.account,
                        amount: reimbursed_amount,
                        created_at_time: 999,
                    },
                ]
            );
            assert_eq!(
                withdrawal_status(burn_index),
                WithdrawSplStatus::FeeReimbursementPending { reimbursed_amount }
            );
            assert!(withdrawal_guard(request.account).is_ok());
        }
        assert_replay();
    }

    #[test]
    fn should_keep_unexpected_ledger_errors_out_of_the_refund_queue() {
        let token = setup();
        let events_before = EventsAssert::from_recorded();
        for error in [
            TransferFromError::BadFee {
                expected_fee: 1_u64.into(),
            },
            TransferFromError::TooOld,
            TransferFromError::CreatedInFuture { ledger_time: 1 },
            TransferFromError::Duplicate {
                duplicate_of: 1_u64.into(),
            },
        ] {
            schedule_withdrawal_fee_reimbursement(
                &TestCanisterRuntime::new(),
                42.into(),
                &withdrawal_terms(&token),
                CKSOL_BURN_AMOUNT,
                &SplBurnError::TransferFrom(error),
                LEDGER_FEE,
            );
        }
        assert_nothing_recorded(events_before);
    }

    #[test]
    fn should_skip_zero_refunds_without_underflow() {
        let token = setup();
        let events_before = EventsAssert::from_recorded();
        for burned_amount in [LEDGER_FEE - 1, LEDGER_FEE] {
            schedule_withdrawal_fee_reimbursement(
                &TestCanisterRuntime::new(),
                42.into(),
                &withdrawal_terms(&token),
                burned_amount,
                &SplBurnError::TransferFrom(TransferFromError::InsufficientFunds {
                    balance: 0_u64.into(),
                }),
                LEDGER_FEE,
            );
        }
        assert_nothing_recorded(events_before);
    }

    #[test]
    #[should_panic(expected = "already recorded SPL withdrawal request as failed")]
    fn should_not_schedule_a_second_refund_for_the_same_fee_burn() {
        let token = setup();
        queue_fee_reimbursement(&token, 42);
        queue_fee_reimbursement(&token, 42);
    }

    #[test]
    #[should_panic(expected = "has no transfer outcome")]
    fn should_reject_an_unused_rent_refund_without_a_transfer_outcome() {
        let token = setup();
        queue_fee_reimbursement(&token, 42);
        record(EventType::QueuedSplCkSolRefund {
            cksol_burn_block_index: 43.into(),
            kind: SplRefundKind::UnusedRent,
            to: account(1),
            amount: 1,
            created_at_time: 999,
        });
    }

    #[tokio::test]
    #[should_panic(expected = "with another kind")]
    async fn should_reject_the_completion_of_a_refund_with_another_kind() {
        queued_fee_reimbursement();
        record(EventType::ReimbursedSplCkSol {
            cksol_burn_block_index: 42.into(),
            kind: SplRefundKind::UnusedRent,
            mint_block_index: 8.into(),
        });
    }

    #[tokio::test]
    #[should_panic(expected = "with another kind")]
    async fn should_reject_the_quarantine_of_a_refund_with_another_kind() {
        queued_fee_reimbursement();
        record(EventType::QuarantinedSplCkSolRefund {
            cksol_burn_block_index: 42.into(),
            kind: SplRefundKind::FailedTransferRent,
        });
    }

    #[test]
    #[should_panic(expected = "has no transfer outcome")]
    fn should_reject_a_failed_transfer_rent_refund_without_a_transfer_outcome() {
        let token = setup();
        queue_fee_reimbursement(&token, 42);
        record(EventType::QueuedSplCkSolRefund {
            cksol_burn_block_index: 43.into(),
            kind: SplRefundKind::FailedTransferRent,
            to: account(1),
            amount: 1,
            created_at_time: 999,
        });
    }

    #[test]
    #[should_panic(expected = "whose token burn did not fail")]
    fn should_reject_a_fee_refund_for_a_withdrawal_whose_token_burn_did_not_fail() {
        setup();
        record(EventType::QueuedSplCkSolRefund {
            cksol_burn_block_index: 42.into(),
            kind: SplRefundKind::FailedTokenBurn,
            to: account(1),
            amount: 1,
            created_at_time: 999,
        });
    }

    #[test]
    #[should_panic(expected = "second ckSOL refund")]
    fn should_reject_a_second_refund_for_the_same_withdrawal() {
        let token = setup();
        let pending = queue_fee_reimbursement(&token, 42);
        record(EventType::QueuedSplCkSolRefund {
            cksol_burn_block_index: 42.into(),
            kind: pending.kind,
            to: pending.to,
            amount: pending.amount,
            created_at_time: pending.created_at_time,
        });
    }

    #[test]
    fn should_build_fee_reimbursement_mint_arguments_from_the_recorded_request() {
        let pending = queued_fee_reimbursement();
        assert_eq!(
            cksol_refund_transfer_arg(42, &pending),
            TransferArg {
                from_subaccount: None,
                to: pending.to,
                fee: None,
                created_at_time: Some(999),
                memo: Some(
                    Memo::from(MintMemo::spl_cksol_refund(
                        42,
                        SplRefundKind::FailedTokenBurn
                    ))
                    .into()
                ),
                amount: CKSOL_BURN_AMOUNT.into(),
            }
        );
    }

    #[tokio::test]
    async fn should_reimburse_a_pending_fee_with_fixed_arguments() {
        let pending = queued_fee_reimbursement();
        let runtime = reimbursement_runtime([Ok(Nat::from(8_u64))]);

        process_pending_cksol_refunds(runtime.clone()).await;

        let calls = runtime.sent_update_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].canister_id, ledger_canister_id());
        assert_eq!(calls[0].method, "icrc1_transfer");
        assert_eq!(
            calls[0].single_arg::<TransferArg>(),
            cksol_refund_transfer_arg(42, &pending)
        );
        assert_fee_reimbursed(42, 8);
        read_state(|state| {
            assert_eq!(
                state.completed_spl_cksol_refunds()[&42],
                CompletedSplCkSolRefund {
                    refund: pending.clone(),
                    mint_block_index: 8.into(),
                    reimbursed_at: 1_000,
                }
            );
        });
        assert_eq!(
            withdrawal_status(42),
            WithdrawSplStatus::FeeReimbursed {
                reimbursed_amount: CKSOL_BURN_AMOUNT,
                mint_block_index: 8,
            }
        );
        assert_replay();
        // A reimbursed fee is not minted again.
        let idle = reimbursement_runtime([]);
        process_pending_cksol_refunds(idle.clone()).await;
        assert!(idle.sent_update_calls().is_empty());
    }

    #[tokio::test]
    async fn should_record_a_duplicate_reply_as_reimbursed() {
        queued_fee_reimbursement();
        let runtime = reimbursement_runtime([Err(TransferError::Duplicate {
            duplicate_of: 9_u64.into(),
        })]);
        process_pending_cksol_refunds(runtime).await;
        assert_fee_reimbursed(42, 9);
    }

    #[tokio::test]
    async fn should_reimburse_every_pending_fee_in_one_round() {
        let token = setup();
        for burn_index in [42, 43, 44] {
            queue_fee_reimbursement(&token, burn_index);
        }
        let runtime = reimbursement_runtime((0..3_u64).map(|index| Ok(Nat::from(index))));
        process_pending_cksol_refunds(runtime.clone()).await;
        assert_eq!(runtime.sent_update_calls().len(), 3);
        for (mint_block_index, burn_index) in [42, 43, 44].into_iter().enumerate() {
            assert_fee_reimbursed(burn_index, mint_block_index as u64);
        }
    }

    #[tokio::test]
    async fn should_keep_the_reimbursement_pending_after_a_transient_failure() {
        let pending = queued_fee_reimbursement();
        let runtime = reimbursement_runtime([
            Err(TransferError::TemporarilyUnavailable),
            Ok(Nat::from(8_u64)),
        ]);

        process_pending_cksol_refunds(runtime.clone()).await;
        read_state(|state| {
            assert_eq!(state.pending_spl_cksol_refunds()[&42], pending);
        });
        process_pending_cksol_refunds(runtime.clone()).await;

        // Both attempts send the same arguments, so the ledger removes a duplicate.
        let calls = runtime.sent_update_calls();
        assert_eq!(calls.len(), 2);
        assert_eq!(
            calls[0].single_arg::<TransferArg>(),
            calls[1].single_arg::<TransferArg>()
        );
        assert_fee_reimbursed(42, 8);
    }

    #[tokio::test]
    async fn should_quarantine_an_expired_reimbursement_without_calling_the_ledger() {
        let pending = queued_fee_reimbursement();
        let expired = 999 + LEDGER_DEDUPLICATION_WINDOW.as_nanos() as u64 + 1;
        let runtime = TestCanisterRuntime::new().with_increasing_time_from(expired);
        process_pending_cksol_refunds(runtime.clone()).await;
        assert!(runtime.sent_update_calls().is_empty());
        assert_fee_quarantined(42, pending);
        assert_eq!(
            withdrawal_status(42),
            WithdrawSplStatus::FeeReimbursementQuarantined {
                reimbursed_amount: CKSOL_BURN_AMOUNT,
            }
        );
        assert_replay();
    }

    #[tokio::test]
    async fn should_quarantine_a_reimbursement_the_ledger_rejects_as_too_old() {
        let pending = queued_fee_reimbursement();
        let runtime = reimbursement_runtime([Err(TransferError::TooOld)]);
        process_pending_cksol_refunds(runtime).await;
        assert_fee_quarantined(42, pending);
    }

    #[tokio::test]
    async fn should_skip_the_round_while_another_one_runs() {
        let pending = queued_fee_reimbursement();
        let _guard = TimerGuard::new(TaskType::RefundSplCkSol).unwrap();
        let runtime = reimbursement_runtime([]);
        process_pending_cksol_refunds(runtime.clone()).await;
        assert!(runtime.sent_update_calls().is_empty());
        read_state(|state| {
            assert_eq!(state.pending_spl_cksol_refunds()[&42], pending);
        });
    }
}

mod withdrawal {
    use super::*;

    #[tokio::test]
    async fn should_report_a_shared_withdrawal_guard_conflict_before_either_burn() {
        let token = setup();
        let request = withdrawal_terms(&token);
        let events_before = EventsAssert::from_recorded();
        // The guard is shared with SOL withdrawals of the same account.
        let _guard = withdrawal_guard(request.account).unwrap();
        let runtime = TestCanisterRuntime::new();

        assert_eq!(
            withdraw_request(&runtime, &request).await,
            Err(WithdrawSplError::AlreadyProcessing)
        );

        assert!(runtime.sent_update_calls().is_empty());
        assert_nothing_recorded(events_before);
    }

    #[tokio::test]
    async fn should_use_the_cksol_burn_index_as_the_withdrawal_id_across_mints() {
        let token = setup();
        record(EventType::Upgrade(UpgradeArgs {
            add_spl_tokens: Some(vec![SplTokenRegistration {
                mint: Address::from([4; 32]).to_string(),
                token_program: spl_token_interface::id().to_string(),
                decimals: 9,
                ledger_id: Principal::from_slice(&[4, 1]),
                index_id: None,
                symbol: "ckOther".to_string(),
                minimum_deposit_amount: 1,
                deposit_fee: 0,
                deposits_paused: false,
                withdrawal_fee_token: None,
            }]),
            ..Default::default()
        }));
        let first = withdrawal_terms(&token);
        let second = SplWithdrawalRequestTerms {
            mint: [4; 32].into(),
            ledger_id: Principal::from_slice(&[4, 1]),
            ..first.clone()
        };
        // Both token ledgers return the same block index for their own burn.
        let runtime = TestCanisterRuntime::new()
            .add_stub_response(burned(42))
            .add_stub_response(burned(7))
            .add_stub_response(burned(43))
            .add_stub_response(burned(7))
            .add_times([100; 10]);

        withdraw_request(&runtime, &first).await.unwrap();
        withdraw_request(&runtime, &second).await.unwrap();

        read_state(|state| {
            let pending = state.pending_spl_withdrawal_requests();
            assert_eq!(pending.len(), 2);
            assert_eq!(pending[&42.into()].request.terms.mint, first.mint);
            assert_eq!(pending[&43.into()].request.terms.mint, second.mint);
        });
        assert_replay();
    }
}

mod ledger_errors {
    use super::*;

    #[test]
    fn should_preserve_ledger_amounts_larger_than_u64_in_spl_burn_errors() {
        let ledger = CkLedger {
            id: Principal::from_slice(&[3, 1]),
            token_symbol: "ckTest".to_string(),
        };
        let large = Nat::from(u128::MAX);
        assert_eq!(
            ckspl_burn_error(
                SplBurnError::TransferFrom(TransferFromError::InsufficientFunds {
                    balance: large.clone()
                }),
                7,
                &ledger
            ),
            LedgerError::InsufficientFunds {
                balance: large.clone(),
                failed_burn_amount: 7_u64.into(),
                token_symbol: "ckTest".to_string(),
                ledger_id: ledger.id,
            }
        );
        assert_eq!(
            ckspl_burn_error(
                SplBurnError::TransferFrom(TransferFromError::BadBurn {
                    min_burn_amount: large.clone()
                }),
                7,
                &ledger
            ),
            LedgerError::AmountTooLow {
                minimum_burn_amount: large,
                failed_burn_amount: 7_u64.into(),
                token_symbol: "ckTest".to_string(),
                ledger_id: ledger.id,
            }
        );
        assert!(matches!(
            ckspl_burn_error(SplBurnError::Call(IcError::CallPerformFailed), 7, &ledger),
            LedgerError::TemporarilyUnavailable(message) if message.starts_with("ckTest ledger: ")
        ));
    }

    #[test]
    fn should_name_the_cksol_ledger_of_mainnet_in_fee_burn_errors() {
        setup();
        assert_eq!(
            cksol_ledger(),
            CkLedger {
                id: ledger_canister_id(),
                token_symbol: "ckSOL".to_string(),
            }
        );
    }
}

mod event_encoding {
    use super::*;

    /// The CBOR tag of the event type: the first element of its two-element array.
    fn tag(payload: EventType) -> u32 {
        let encoded = minicbor::to_vec(payload).unwrap();
        let mut decoder = minicbor::Decoder::new(&encoded);
        assert_eq!(decoder.array().unwrap(), Some(2));
        decoder.u32().unwrap()
    }

    #[test]
    fn should_preserve_the_event_tags_of_the_withdrawal_request() {
        let token = setup();
        let kind = SplRefundKind::FailedTokenBurn;
        for (payload, expected) in [
            (
                EventType::AcceptedSplWithdrawalRequest(SplWithdrawalRequest {
                    terms: withdrawal_terms(&token),
                    cksol_burn_block_index: 42.into(),
                    ckspl_burn_block_index: 7.into(),
                }),
                25,
            ),
            (
                EventType::FailedSplWithdrawalRequest {
                    cksol_burn_block_index: 42.into(),
                    to: account(1),
                    reimbursed_amount: 1,
                },
                26,
            ),
            (
                EventType::QueuedSplCkSolRefund {
                    cksol_burn_block_index: 42.into(),
                    kind,
                    to: account(1),
                    amount: 1,
                    created_at_time: 999,
                },
                30,
            ),
            (
                EventType::ReimbursedSplCkSol {
                    cksol_burn_block_index: 42.into(),
                    kind,
                    mint_block_index: 8.into(),
                },
                31,
            ),
            (
                EventType::QuarantinedSplCkSolRefund {
                    cksol_burn_block_index: 42.into(),
                    kind,
                },
                32,
            ),
        ] {
            assert_eq!(tag(payload), expected);
        }
    }
}
