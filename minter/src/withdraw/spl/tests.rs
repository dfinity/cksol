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

fn halt() {
    record(EventType::HaltedSplWithdrawals {
        signature: solana_signature::Signature::from([9; 64]),
        rent_spent: RENT_RESERVE + 1,
        rent_reserve: RENT_RESERVE,
    });
}

fn set_halted(halted: bool) {
    record(EventType::Upgrade(UpgradeArgs {
        spl_withdrawals_halted: Some(halted),
        ..Default::default()
    }));
}

fn halted() -> bool {
    read_state(|state| state.spl_withdrawals_halted())
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

    fn record_fee_refund_after_failed_request(to: Account, amount: Lamport) {
        setup();
        record(EventType::FailedSplWithdrawalRequest {
            cksol_burn_block_index: 42.into(),
            to: account(1),
            reimbursed_amount: 100,
        });
        record(EventType::QueuedSplCkSolRefund {
            cksol_burn_block_index: 42.into(),
            kind: SplRefundKind::FailedTokenBurn,
            to,
            amount,
            created_at_time: 999,
        });
    }

    #[test]
    fn should_queue_the_fee_refund_that_the_failed_request_recorded() {
        record_fee_refund_after_failed_request(account(1), 100);

        read_state(|state| {
            let pending = &state.pending_spl_cksol_refunds()[&42];
            assert_eq!((pending.to, pending.amount), (account(1), 100));
        });
    }

    #[test]
    #[should_panic(expected = "another account or amount than recorded")]
    fn should_reject_a_fee_refund_to_another_account_than_the_failed_request() {
        record_fee_refund_after_failed_request(account(2), 100);
    }

    #[test]
    #[should_panic(expected = "another account or amount than recorded")]
    fn should_reject_a_fee_refund_of_another_amount_than_the_failed_request() {
        record_fee_refund_after_failed_request(account(1), 99);
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

mod halt {
    use super::*;

    #[tokio::test]
    async fn should_reject_a_new_withdrawal_before_either_burn_while_halted() {
        let token = setup();
        let request = withdrawal_terms(&token);
        halt();
        let events_before = EventsAssert::from_recorded();
        let runtime = TestCanisterRuntime::new();

        assert_eq!(
            withdraw_request(&runtime, &request).await,
            Err(WithdrawSplError::WithdrawalsHalted)
        );

        assert!(runtime.sent_update_calls().is_empty());
        assert_nothing_recorded(events_before);
        assert!(withdrawal_guard(request.account).is_ok());
    }

    #[tokio::test]
    async fn should_accept_a_withdrawal_whose_burns_ran_when_the_halt_began() {
        // The halt begins during the ckSOL burn (1) or during the ckSPL burn (2).
        for burn_in_progress in [1, 2] {
            let token = setup();
            let request = withdrawal_terms(&token);
            let runtime = TestCanisterRuntime::new()
                .add_stub_response(burned(42))
                .add_stub_response(burned(7))
                .add_times([123; 10]);
            let withdrawal = withdraw_request(&runtime, &request);
            tokio::pin!(withdrawal);
            while runtime.sent_update_calls().len() < burn_in_progress {
                assert!(futures::poll!(&mut withdrawal).is_pending());
            }
            halt();

            assert_eq!(
                withdrawal.await,
                Ok(WithdrawSplOk {
                    cksol_burn_block_index: 42,
                    ckspl_burn_block_index: 7,
                })
            );

            assert_eq!(withdrawal_status(42), WithdrawSplStatus::Pending);
            read_state(|state| {
                assert!(state.pending_spl_cksol_refunds().is_empty());
                assert!(
                    state
                        .pending_spl_withdrawal_requests()
                        .contains_key(&42.into())
                );
            });
            assert!(halted());
            assert_replay();
        }
    }

    #[test]
    fn should_keep_the_halt_across_upgrades_until_one_resumes() {
        setup();
        assert!(!halted());
        halt();
        assert!(halted());

        record(EventType::Upgrade(UpgradeArgs::default()));
        assert!(halted());
        set_halted(true);
        assert!(halted());
        assert_replay();

        set_halted(false);
        assert!(!halted());
        assert_replay();
    }

    #[test]
    fn should_halt_by_an_upgrade_argument() {
        setup();

        set_halted(true);

        assert!(halted());
        assert_replay();
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

mod transactions {
    use super::*;
    use crate::{
        address::account_address,
        constants::{FEE_PER_SIGNATURE, MAX_CONCURRENT_SIGNATURES, RENT_EXEMPTION_THRESHOLD},
        spl_transfer::{build_spl_withdrawal_message, withdrawal::SplWithdrawalTransfer},
        state::{
            MinterTransaction, QueuedSplDeposit, SPL_SWEEP_TRANSACTION_FEE, SplSweep,
            WithdrawalPlan,
            event::{Signer, TransactionPurpose, WithdrawalRequest},
        },
        test_fixtures::{
            DEFAULT_BLOCK_HEIGHT, durable_nonce, init_balance_to, minter_signature,
            nonce_account_info, schnorr_master_key, signature, signer::sign_as_minter,
        },
        withdraw::process_pending_withdrawals,
    };
    use ic_cdk_management_canister::SignCallError;
    use sol_rpc_types::MultiRpcResult;

    type GetAccountInfoResult = MultiRpcResult<Option<sol_rpc_types::AccountInfo>>;
    type SendTransactionResult = MultiRpcResult<sol_rpc_types::Signature>;

    const BALANCE: Lamport = 500_000_000;
    const TOKENS: u64 = 10_000_000;
    /// The SOL that one SPL withdrawal transaction reserves: its fee and the rent.
    const SOL_COST: Lamport = FEE_PER_SIGNATURE + RENT_RESERVE;
    /// The part of the balance that the main address keeps for its own rent.
    const HELD_BACK: Lamport = RENT_EXEMPTION_THRESHOLD;

    /// Queues the sweep of an SPL deposit of `tokens` and returns its plan.
    fn queue_sweep(deposit_id: u64, tokens: u64) -> SplSweep {
        let account = account(100 + deposit_id as usize);
        let deposit = QueuedSplDeposit {
            account,
            mint: mint(),
            address: associated_token_address(
                &account_address(&schnorr_master_key(), &account),
                &mint(),
            ),
            balance: tokens,
            fee: 10,
        };
        record(EventType::RequestedOperatingRefill {
            refill_id: deposit_id,
            amount: SPL_SWEEP_TRANSACTION_FEE,
            created_at_time: 100,
        });
        record(EventType::CompletedOperatingRefill {
            refill_id: deposit_id,
            burn_block_index: 1_000 + deposit_id,
        });
        record(EventType::QueuedSplDeposit {
            deposit_id,
            account,
            mint: deposit.mint,
            address: deposit.address,
            balance: deposit.balance,
            fee: deposit.fee,
        });
        read_state(|state| {
            SplSweep::plan(
                deposit_id,
                deposit,
                state.supported_spl_token(&mint()).unwrap(),
                &schnorr_master_key(),
            )
            .unwrap()
        })
    }

    fn submit_sweep(sweep: &SplSweep) -> solana_signature::Signature {
        let sweep_signature = signature(0x5B00 + sweep.deposit_id() as usize);
        record(EventType::SubmittedTransaction {
            signature: sweep_signature,
            message: sweep.sweep_message(Hash::default()).into(),
            signers: sweep.signers().to_vec(),
            purpose: TransactionPurpose::SweepSplDeposit {
                deposit_id: sweep.deposit_id(),
                block_height: DEFAULT_BLOCK_HEIGHT,
            },
        });
        sweep_signature
    }

    /// Sets the main balance to `balance` and the treasury of [`mint`] to `tokens`, with
    /// a credited sweep of one deposit.
    fn setup_funded(balance: Lamport, tokens: u64) -> SupportedSplToken {
        let token = setup();
        init_balance_to(balance + SPL_SWEEP_TRANSACTION_FEE);
        let sweep = queue_sweep(0, tokens);
        let sweep_signature = submit_sweep(&sweep);
        record(EventType::SucceededTransaction {
            signature: sweep_signature,
        });
        record(EventType::CreditedSplSweep {
            signature: sweep_signature,
            lamports_spent: SPL_SWEEP_TRANSACTION_FEE,
            deposit_id: 0,
            amount_to_mint: tokens - 10,
        });
        read_state(|state| {
            assert_eq!(state.balance(), balance);
            assert_eq!(state.spl_treasury_balance(&mint()), tokens);
        });
        token
    }

    fn queue_withdrawal(token: &SupportedSplToken, id: u64, amount: u64) -> SplWithdrawalRequest {
        let request = SplWithdrawalRequest {
            terms: SplWithdrawalRequestTerms {
                amount,
                ..withdrawal_terms(token)
            },
            cksol_burn_block_index: id.into(),
            ckspl_burn_block_index: (1_000 + id).into(),
        };
        record(EventType::AcceptedSplWithdrawalRequest(request.clone()));
        request
    }

    fn queue_sol_withdrawal(id: u64, amount_to_transfer: Lamport) {
        record(EventType::AcceptedWithdrawalRequest(WithdrawalRequest {
            account: account(1),
            solana_address: [0; 32],
            burn_block_index: id.into(),
            burned_amount: amount_to_transfer + 1,
            amount_to_transfer,
        }));
    }

    fn nonce_read() -> GetAccountInfoResult {
        MultiRpcResult::Consistent(Ok(Some(nonce_account_info(MINTER_ADDRESS, 1))))
    }

    fn sent() -> SendTransactionResult {
        MultiRpcResult::Consistent(Ok(minter_signature().into()))
    }

    fn plans() -> Vec<WithdrawalPlan> {
        read_state(|state| state.withdrawal_plans().collect())
    }

    fn sol_plan(ids: &[u64]) -> Vec<LedgerBurnIndex> {
        ids.iter().copied().map(LedgerBurnIndex::new).collect()
    }

    /// The burn indices of each plan: one for an SPL plan, the batch for a SOL plan.
    fn plan_ids(plans: &[WithdrawalPlan]) -> Vec<(&'static str, Vec<LedgerBurnIndex>)> {
        plans
            .iter()
            .map(|plan| match plan {
                WithdrawalPlan::Sol(requests) => {
                    ("sol", requests.iter().map(|r| r.burn_block_index).collect())
                }
                WithdrawalPlan::Spl(request) => ("spl", vec![request.cksol_burn_block_index]),
            })
            .collect()
    }

    #[tokio::test]
    async fn should_create_sign_and_send_an_spl_withdrawal_in_one_round() {
        let token = setup_funded(BALANCE, TOKENS);
        let request = queue_withdrawal(&token, 42, 1_000);
        let events_before = EventsAssert::from_recorded();
        let runtime = TestCanisterRuntime::new()
            .add_times([200; 20])
            .add_stub_response(nonce_read())
            .add_stub_response(sent())
            .add_signer(sign_as_minter());

        process_pending_withdrawals(runtime.clone()).await;

        let expected_message = build_spl_withdrawal_message(
            &MINTER_ADDRESS,
            &NONCE_ACCOUNT,
            durable_nonce(1),
            &SplWithdrawalTransfer {
                token: &token,
                destination: &request.terms.address,
                amount: 1_000,
            },
        )
        .unwrap();
        let events_after = EventsAssert::from_recorded();
        assert_eq!(events_after.len(), events_before.len() + 2);
        events_after
            .expect_contains_event_eq(EventType::CreatedSplWithdrawalTransaction {
                cksol_burn_block_index: 42.into(),
                nonce_account: NONCE_ACCOUNT,
                nonce_value: durable_nonce(1),
            })
            .expect_contains_event_eq(EventType::SubmittedTransaction {
                signature: minter_signature(),
                message: expected_message.clone().into(),
                signers: vec![Signer::Minter],
                purpose: TransactionPurpose::SplWithdrawal {
                    cksol_burn_block_index: 42.into(),
                },
            });
        assert_eq!(
            runtime
                .sent_update_calls()
                .iter()
                .map(|call| call.method.as_str())
                .collect::<Vec<_>>(),
            ["getAccountInfo", "sendTransaction"]
        );
        read_state(|state| {
            assert_eq!(
                state.submitted_transactions().get(&minter_signature()),
                Some(&MinterTransaction::SplWithdrawal {
                    message: expected_message.into(),
                    nonce_account: NONCE_ACCOUNT,
                    nonce_value: durable_nonce(1),
                    submitted_at: 200,
                })
            );
            assert!(state.pending_spl_withdrawal_requests().is_empty());
            assert!(state.created_spl_withdrawal_requests().is_empty());
            assert!(state.created_spl_withdrawal_txs().is_empty());
            let sent = &state.sent_spl_withdrawal_requests()[&42.into()];
            assert_eq!(sent.request, request);
            assert_eq!(sent.signature, minter_signature());
            // The fee and the rent, and the transferred tokens, are reserved.
            assert_eq!(state.balance(), BALANCE - SOL_COST);
            assert_eq!(state.spl_treasury_balance(&mint()), TOKENS - 1_000);
            assert_eq!(state.nonce_pool().num_free_accounts(), 0);
        });
        assert_eq!(
            withdrawal_status(42),
            WithdrawSplStatus::TxSent {
                transaction_id: minter_signature().into()
            }
        );
        assert_replay();
    }

    #[tokio::test]
    async fn should_transfer_the_amount_less_the_token_fee_and_keep_the_fee_in_the_treasury() {
        let token = setup_funded(BALANCE, TOKENS);
        let request = SplWithdrawalRequest {
            terms: SplWithdrawalRequestTerms {
                amount: 1_000,
                token_fee: 25,
                ..withdrawal_terms(&token)
            },
            cksol_burn_block_index: 42.into(),
            ckspl_burn_block_index: 7.into(),
        };
        record(EventType::AcceptedSplWithdrawalRequest(request.clone()));
        let runtime = TestCanisterRuntime::new()
            .add_times([200; 20])
            .add_stub_response(nonce_read())
            .add_stub_response(sent())
            .add_signer(sign_as_minter());

        process_pending_withdrawals(runtime).await;

        let expected_message = build_spl_withdrawal_message(
            &MINTER_ADDRESS,
            &NONCE_ACCOUNT,
            durable_nonce(1),
            &SplWithdrawalTransfer {
                token: &token,
                destination: &request.terms.address,
                amount: 975,
            },
        )
        .unwrap();
        read_state(|state| {
            assert!(matches!(
                state.submitted_transactions().get(&minter_signature()),
                Some(MinterTransaction::SplWithdrawal { message, .. })
                    if *message == expected_message.clone().into()
            ));
            assert_eq!(state.spl_treasury_balance(&mint()), TOKENS - 975);
        });
        assert_replay();
    }

    #[test]
    fn should_order_plans_by_burn_index_across_both_kinds() {
        let token = setup_funded(BALANCE, TOKENS);
        queue_withdrawal(&token, 100, 1_000);
        queue_sol_withdrawal(101, 1_000_000);
        queue_withdrawal(&token, 102, 1_000);
        queue_sol_withdrawal(103, 1_000_000);
        queue_sol_withdrawal(104, 1_000_000);
        // A SOL batch never serves a request newer than an SPL request still waiting.
        assert_eq!(
            plan_ids(&plans()),
            [
                ("spl", sol_plan(&[100])),
                ("sol", sol_plan(&[101])),
                ("spl", sol_plan(&[102])),
                ("sol", sol_plan(&[103, 104])),
            ]
        );
    }

    #[test]
    fn should_skip_an_unfunded_spl_request_without_blocking_other_requests() {
        let token = setup_funded(BALANCE, 1_500);
        // The treasury covers the first and the third request, not the second.
        queue_withdrawal(&token, 100, 1_000);
        queue_withdrawal(&token, 101, 1_000);
        queue_sol_withdrawal(102, 1_000_000);
        queue_withdrawal(&token, 103, 500);
        assert_eq!(
            plan_ids(&plans()),
            [
                ("spl", sol_plan(&[100])),
                ("sol", sol_plan(&[102])),
                ("spl", sol_plan(&[103])),
            ]
        );
    }

    #[test]
    fn should_not_block_spl_requests_behind_a_sol_request_without_funds() {
        let small_sol_cost = 1 + FEE_PER_SIGNATURE;
        let token = setup_funded(HELD_BACK + SOL_COST + small_sol_cost, TOKENS);
        // The balance covers one SPL withdrawal and the small SOL withdrawal 102, and
        // not the large SOL withdrawal 100.
        queue_sol_withdrawal(100, BALANCE);
        queue_withdrawal(&token, 101, 1_000);
        queue_sol_withdrawal(102, 1);
        queue_withdrawal(&token, 103, 1_000);
        // Request 102 is affordable, but the SOL queue keeps its order behind request
        // 100. The second SPL request has too little SOL left for its fee and rent.
        assert_eq!(plan_ids(&plans()), [("spl", sol_plan(&[101]))]);
    }

    #[test]
    fn should_share_the_sol_budget_between_both_kinds() {
        let token = setup_funded(HELD_BACK + SOL_COST + 1_000 + FEE_PER_SIGNATURE, TOKENS);
        queue_sol_withdrawal(100, 1_000);
        queue_withdrawal(&token, 101, 1_000);
        queue_withdrawal(&token, 102, 1_000);
        assert_eq!(
            plan_ids(&plans()),
            [("sol", sol_plan(&[100])), ("spl", sol_plan(&[101]))]
        );
    }

    #[tokio::test]
    async fn should_leave_an_unfunded_spl_withdrawal_pending_without_a_nonce_read() {
        for (balance, tokens) in [(BALANCE, 999), (HELD_BACK + SOL_COST - 1, TOKENS)] {
            let token = setup_funded(balance, tokens);
            queue_withdrawal(&token, 42, 1_000);
            let events_before = EventsAssert::from_recorded();
            let runtime = TestCanisterRuntime::new().add_times([200; 20]);

            process_pending_withdrawals(runtime.clone()).await;

            assert!(runtime.sent_update_calls().is_empty());
            assert_eq!(EventsAssert::from_recorded(), events_before);
            assert_eq!(withdrawal_status(42), WithdrawSplStatus::Pending);
        }
    }

    #[tokio::test]
    async fn should_recheck_the_funds_of_an_spl_withdrawal_after_the_nonce_read() {
        let token = setup_funded(HELD_BACK + SOL_COST, TOKENS);
        queue_withdrawal(&token, 42, 1_000);
        let sweep = queue_sweep(1, 1_000);
        let events_before = EventsAssert::from_recorded();
        let runtime = TestCanisterRuntime::new()
            .add_times([200; 20])
            .add_stub_response(nonce_read());
        let processing = process_pending_withdrawals(runtime.clone());
        tokio::pin!(processing);
        // The mocked RPC runtime suspends once per call. Reserve during the nonce read.
        while runtime.sent_update_calls().is_empty() {
            assert!(futures::poll!(&mut processing).is_pending());
        }
        submit_sweep(&sweep);
        processing.await;

        // Only the sweep submission is recorded. No transaction is created.
        assert_eq!(EventsAssert::from_recorded().len(), events_before.len() + 1);
        assert_eq!(withdrawal_status(42), WithdrawSplStatus::Pending);
        read_state(|state| {
            assert_eq!(state.nonce_pool().num_free_accounts(), 1);
            assert_eq!(state.spl_treasury_balance(&mint()), TOKENS);
        });
    }

    #[tokio::test]
    async fn should_queue_requests_while_the_nonce_pool_is_busy() {
        let token = setup_funded(BALANCE, TOKENS);
        queue_withdrawal(&token, 42, 1_000);
        queue_withdrawal(&token, 43, 1_000);
        let runtime = TestCanisterRuntime::new()
            .add_times([200; 20])
            .add_stub_response(nonce_read())
            .add_stub_response(sent())
            .add_signer(sign_as_minter());

        process_pending_withdrawals(runtime.clone()).await;

        // The pool has one account, so the older request takes it and the other waits.
        assert!(matches!(
            withdrawal_status(42),
            WithdrawSplStatus::TxSent { .. }
        ));
        assert_eq!(withdrawal_status(43), WithdrawSplStatus::Pending);
        let idle = TestCanisterRuntime::new().add_times([300; 20]);
        process_pending_withdrawals(idle.clone()).await;
        assert!(idle.sent_update_calls().is_empty());
        assert_eq!(withdrawal_status(43), WithdrawSplStatus::Pending);
    }

    #[tokio::test]
    async fn should_re_sign_the_identical_spl_message_after_a_signing_failure() {
        let token = setup_funded(BALANCE, TOKENS);
        queue_withdrawal(&token, 42, 1_000);
        let failing = TestCanisterRuntime::new()
            .add_times([200; 20])
            .add_stub_response(nonce_read())
            .add_signer(sign_as_minter().expect([Err(SignCallError::CallFailed(
                ic_cdk::call::CallPerformFailed.into(),
            ))]));

        process_pending_withdrawals(failing.clone()).await;

        // The binding is recorded and the funds are reserved, but nothing is submitted.
        assert_eq!(withdrawal_status(42), WithdrawSplStatus::Pending);
        let bound = read_state(|state| {
            assert!(state.submitted_transactions().is_empty());
            assert_eq!(state.balance(), BALANCE - SOL_COST);
            assert_eq!(state.spl_treasury_balance(&mint()), TOKENS - 1_000);
            let created = state.created_spl_withdrawal_txs()[&NONCE_ACCOUNT].clone();
            state
                .bound_spl_withdrawal_message(&NONCE_ACCOUNT, &created)
                .unwrap()
        });
        assert_replay();

        let retry = TestCanisterRuntime::new()
            .add_times([300; 20])
            .add_stub_response(sent())
            .add_signer(sign_as_minter());
        process_pending_withdrawals(retry.clone()).await;

        // The second round reads no nonce: it signs the message bound in the first round.
        assert_eq!(
            retry
                .sent_update_calls()
                .iter()
                .map(|call| call.method.as_str())
                .collect::<Vec<_>>(),
            ["sendTransaction"]
        );
        read_state(|state| {
            assert!(matches!(
                state.submitted_transactions().get(&minter_signature()),
                Some(MinterTransaction::SplWithdrawal { message, .. }) if *message == bound.clone().into()
            ));
            assert_eq!(state.balance(), BALANCE - SOL_COST);
        });
        assert_replay();
    }

    #[test]
    #[should_panic(expected = "is settled by its own events")]
    fn should_reject_the_sol_success_event_for_an_spl_withdrawal_transaction() {
        let token = setup_funded(BALANCE, TOKENS);
        let request = queue_withdrawal(&token, 42, 1_000);
        record(EventType::CreatedSplWithdrawalTransaction {
            cksol_burn_block_index: 42.into(),
            nonce_account: NONCE_ACCOUNT,
            nonce_value: durable_nonce(1),
        });
        let message = build_spl_withdrawal_message(
            &MINTER_ADDRESS,
            &NONCE_ACCOUNT,
            durable_nonce(1),
            &SplWithdrawalTransfer {
                token: &token,
                destination: &request.terms.address,
                amount: 1_000,
            },
        )
        .unwrap();
        record(EventType::SubmittedTransaction {
            signature: signature(1),
            message: message.into(),
            signers: vec![Signer::Minter],
            purpose: TransactionPurpose::SplWithdrawal {
                cksol_burn_block_index: 42.into(),
            },
        });
        record(EventType::SucceededTransaction {
            signature: signature(1),
        });
    }

    #[test]
    #[should_panic(expected = "is settled by its own events")]
    fn should_reject_the_sol_failure_event_for_an_spl_withdrawal_transaction() {
        let token = setup_funded(BALANCE, TOKENS);
        let request = queue_withdrawal(&token, 42, 1_000);
        record(EventType::CreatedSplWithdrawalTransaction {
            cksol_burn_block_index: 42.into(),
            nonce_account: NONCE_ACCOUNT,
            nonce_value: durable_nonce(1),
        });
        let message = build_spl_withdrawal_message(
            &MINTER_ADDRESS,
            &NONCE_ACCOUNT,
            durable_nonce(1),
            &SplWithdrawalTransfer {
                token: &token,
                destination: &request.terms.address,
                amount: 1_000,
            },
        )
        .unwrap();
        record(EventType::SubmittedTransaction {
            signature: signature(1),
            message: message.into(),
            signers: vec![Signer::Minter],
            purpose: TransactionPurpose::SplWithdrawal {
                cksol_burn_block_index: 42.into(),
            },
        });
        record(EventType::FailedTransaction {
            signature: signature(1),
        });
    }

    #[test]
    #[should_panic(expected = "does not carry the message bound to nonce account")]
    fn should_reject_a_submission_with_another_message_than_the_bound_one() {
        let token = setup_funded(BALANCE, TOKENS);
        let request = queue_withdrawal(&token, 42, 1_000);
        record(EventType::CreatedSplWithdrawalTransaction {
            cksol_burn_block_index: 42.into(),
            nonce_account: NONCE_ACCOUNT,
            nonce_value: durable_nonce(1),
        });
        let other_amount = build_spl_withdrawal_message(
            &MINTER_ADDRESS,
            &NONCE_ACCOUNT,
            durable_nonce(1),
            &SplWithdrawalTransfer {
                token: &token,
                destination: &request.terms.address,
                amount: 1_001,
            },
        )
        .unwrap();
        record(EventType::SubmittedTransaction {
            signature: signature(1),
            message: other_amount.into(),
            signers: vec![Signer::Minter],
            purpose: TransactionPurpose::SplWithdrawal {
                cksol_burn_block_index: 42.into(),
            },
        });
    }

    #[test]
    fn should_preserve_the_tags_of_the_creation_event_and_the_purpose() {
        let encoded = minicbor::to_vec(EventType::CreatedSplWithdrawalTransaction {
            cksol_burn_block_index: 42.into(),
            nonce_account: NONCE_ACCOUNT,
            nonce_value: durable_nonce(1),
        })
        .unwrap();
        let mut decoder = minicbor::Decoder::new(&encoded);
        assert_eq!(decoder.array().unwrap(), Some(2));
        assert_eq!(decoder.u32().unwrap(), 27);
        assert_eq!(decoder.array().unwrap(), Some(3));
        assert_eq!(decoder.u64().unwrap(), 42);

        let encoded = minicbor::to_vec(TransactionPurpose::SplWithdrawal {
            cksol_burn_block_index: 42.into(),
        })
        .unwrap();
        let mut decoder = minicbor::Decoder::new(&encoded);
        assert_eq!(decoder.array().unwrap(), Some(2));
        assert_eq!(decoder.u32().unwrap(), 6);
        assert_eq!(decoder.array().unwrap(), Some(1));
        assert_eq!(decoder.u64().unwrap(), 42);
    }

    mod halt {
        use super::*;

        #[test]
        fn should_plan_only_sol_withdrawals_while_spl_withdrawals_are_halted() {
            let token = setup_funded(BALANCE, TOKENS);
            queue_withdrawal(&token, 100, 1_000);
            queue_sol_withdrawal(101, 1_000_000);
            queue_withdrawal(&token, 102, 1_000);
            queue_sol_withdrawal(103, 1_000_000);

            halt();

            // With no SPL request in the plans, the SOL requests form one batch.
            assert_eq!(plan_ids(&plans()), [("sol", sol_plan(&[101, 103]))]);
            read_state(|state| assert_eq!(state.pending_spl_withdrawal_requests().len(), 2));

            set_halted(false);

            assert_eq!(
                plan_ids(&plans()),
                [
                    ("spl", sol_plan(&[100])),
                    ("sol", sol_plan(&[101])),
                    ("spl", sol_plan(&[102])),
                    ("sol", sol_plan(&[103])),
                ]
            );
        }

        #[tokio::test]
        async fn should_hold_spl_withdrawals_and_send_sol_withdrawals_while_halted() {
            let token = setup_funded(BALANCE, TOKENS);
            queue_withdrawal(&token, 42, 1_000);
            queue_sol_withdrawal(43, 1_000_000);
            halt();
            let runtime = TestCanisterRuntime::new()
                .add_times([200; 20])
                .add_stub_response(nonce_read())
                .add_stub_response(sent())
                .add_signer(sign_as_minter());

            process_pending_withdrawals(runtime.clone()).await;

            assert_eq!(
                runtime
                    .sent_update_calls()
                    .iter()
                    .map(|call| call.method.as_str())
                    .collect::<Vec<_>>(),
                ["getAccountInfo", "sendTransaction"]
            );
            assert_eq!(withdrawal_status(42), WithdrawSplStatus::Pending);
            read_state(|state| {
                assert!(state.sent_withdrawal_requests().contains_key(&43.into()));
                assert!(state.created_spl_withdrawal_txs().is_empty());
                assert_eq!(state.spl_treasury_balance(&mint()), TOKENS);
            });

            set_halted(false);

            assert_eq!(plan_ids(&plans()), [("spl", sol_plan(&[42]))]);
            assert_replay();
        }

        #[tokio::test]
        async fn should_hold_an_spl_withdrawal_while_halted_without_a_nonce_read() {
            let token = setup_funded(BALANCE, TOKENS);
            queue_withdrawal(&token, 42, 1_000);
            halt();
            let events_before = EventsAssert::from_recorded();
            let runtime = TestCanisterRuntime::new().add_times([200; 20]);

            process_pending_withdrawals(runtime.clone()).await;

            assert!(runtime.sent_update_calls().is_empty());
            assert_eq!(EventsAssert::from_recorded(), events_before);
            assert_eq!(withdrawal_status(42), WithdrawSplStatus::Pending);
        }

        #[tokio::test]
        async fn should_recheck_the_halt_after_the_nonce_read() {
            let token = setup_funded(BALANCE, TOKENS);
            queue_withdrawal(&token, 42, 1_000);
            let events_before = EventsAssert::from_recorded();
            let runtime = TestCanisterRuntime::new()
                .add_times([200; 20])
                .add_stub_response(nonce_read());
            let processing = process_pending_withdrawals(runtime.clone());
            tokio::pin!(processing);
            while runtime.sent_update_calls().is_empty() {
                assert!(futures::poll!(&mut processing).is_pending());
            }
            halt();
            processing.await;

            // Only the halt is recorded. No transaction is created.
            assert_eq!(EventsAssert::from_recorded().len(), events_before.len() + 1);
            assert_eq!(runtime.sent_update_calls().len(), 1);
            assert_eq!(withdrawal_status(42), WithdrawSplStatus::Pending);
            read_state(|state| {
                assert_eq!(state.nonce_pool().num_free_accounts(), 1);
                assert_eq!(state.spl_treasury_balance(&mint()), TOKENS);
            });
        }

        #[tokio::test]
        async fn should_hold_a_created_spl_transaction_while_halted_and_send_it_after_the_resume() {
            let token = setup_funded(BALANCE, TOKENS);
            queue_withdrawal(&token, 42, 1_000);
            let failing = TestCanisterRuntime::new()
                .add_times([200; 20])
                .add_stub_response(nonce_read())
                .add_signer(sign_as_minter().expect([Err(SignCallError::CallFailed(
                    ic_cdk::call::CallPerformFailed.into(),
                ))]));
            process_pending_withdrawals(failing.clone()).await;
            let bound = read_state(|state| {
                let created = state.created_spl_withdrawal_txs()[&NONCE_ACCOUNT].clone();
                state
                    .bound_spl_withdrawal_message(&NONCE_ACCOUNT, &created)
                    .unwrap()
            });
            halt();
            let events_before = EventsAssert::from_recorded();

            // The runtime has no signer, so a signing request fails the test.
            let halted_round = TestCanisterRuntime::new().add_times([300; 20]);
            process_pending_withdrawals(halted_round.clone()).await;

            // The transaction is not signed or sent. Its nonce and its funds stay reserved.
            assert!(halted_round.sent_update_calls().is_empty());
            assert_eq!(EventsAssert::from_recorded(), events_before);
            read_state(|state| {
                assert!(state.submitted_transactions().is_empty());
                assert!(
                    state
                        .created_spl_withdrawal_txs()
                        .contains_key(&NONCE_ACCOUNT)
                );
                assert_eq!(state.nonce_pool().num_free_accounts(), 0);
                assert_eq!(state.balance(), BALANCE - SOL_COST);
                assert_eq!(state.spl_treasury_balance(&mint()), TOKENS - 1_000);
            });

            set_halted(false);
            let resumed = TestCanisterRuntime::new()
                .add_times([400; 20])
                .add_stub_response(sent())
                .add_signer(sign_as_minter());
            process_pending_withdrawals(resumed.clone()).await;

            // After the resume, the round signs and sends the same message. It reads no nonce.
            assert_eq!(
                resumed
                    .sent_update_calls()
                    .iter()
                    .map(|call| call.method.as_str())
                    .collect::<Vec<_>>(),
                ["sendTransaction"]
            );
            read_state(|state| {
                assert!(matches!(
                    state.submitted_transactions().get(&minter_signature()),
                    Some(MinterTransaction::SplWithdrawal { message, .. }) if *message == bound.clone().into()
                ));
            });
            assert_replay();
        }

        #[tokio::test]
        async fn should_send_a_transaction_whose_signing_began_before_the_halt() {
            let token = setup_funded(BALANCE, TOKENS);
            queue_withdrawal(&token, 42, 1_000);
            // The halt begins during the signing call.
            let runtime = TestCanisterRuntime::new()
                .add_times([200; 20])
                .add_stub_response(nonce_read())
                .add_stub_response(sent())
                .add_signer(sign_as_minter().before_reply(halt));

            process_pending_withdrawals(runtime.clone()).await;

            assert!(halted());
            assert_eq!(
                runtime
                    .sent_update_calls()
                    .iter()
                    .map(|call| call.method.as_str())
                    .collect::<Vec<_>>(),
                ["getAccountInfo", "sendTransaction"]
            );
            assert_eq!(
                withdrawal_status(42),
                WithdrawSplStatus::TxSent {
                    transaction_id: minter_signature().into()
                }
            );
            assert_replay();
        }

        #[tokio::test]
        async fn should_send_the_whole_signing_batch_when_the_halt_begins_during_a_later_signature()
        {
            let token = setup_funded(BALANCE, TOKENS);
            let second_nonce = Address::from([101; 32]);
            record(EventType::Upgrade(UpgradeArgs {
                nonce_accounts_to_add: Some(vec![second_nonce.to_string()]),
                ..Default::default()
            }));
            queue_withdrawal(&token, 42, 1_000);
            queue_withdrawal(&token, 43, 1_000);
            let runtime = TestCanisterRuntime::new()
                .add_times([200; 40])
                .add_stub_response(nonce_read())
                .add_stub_response(nonce_read())
                .add_stub_response(sent())
                .add_stub_response(sent())
                .add_signer(sign_as_minter())
                .add_signer(sign_as_minter().before_reply(halt));

            process_pending_withdrawals(runtime.clone()).await;

            assert!(halted());
            assert_eq!(
                runtime
                    .sent_update_calls()
                    .iter()
                    .map(|call| call.method.as_str())
                    .collect::<Vec<_>>(),
                [
                    "getAccountInfo",
                    "getAccountInfo",
                    "sendTransaction",
                    "sendTransaction"
                ]
            );
            read_state(|state| {
                let first = &state.sent_spl_withdrawal_requests()[&42.into()];
                let second = &state.sent_spl_withdrawal_requests()[&43.into()];
                assert_ne!(first.signature, second.signature);
                assert_eq!(state.submitted_transactions().len(), 2);
                assert!(state.created_spl_withdrawal_txs().is_empty());
                assert_eq!(state.nonce_pool().num_free_accounts(), 0);
            });
            assert_replay();
        }

        #[tokio::test]
        async fn should_restore_sol_progress_by_adding_a_nonce_while_spl_is_halted() {
            let token = setup_funded(BALANCE, TOKENS);
            queue_withdrawal(&token, 42, 1_000);
            record(EventType::CreatedSplWithdrawalTransaction {
                cksol_burn_block_index: 42.into(),
                nonce_account: NONCE_ACCOUNT,
                nonce_value: durable_nonce(1),
            });
            queue_sol_withdrawal(43, 1_000_000);
            halt();
            // The held SPL transaction uses the only nonce account, so the SOL withdrawal waits.
            let idle = TestCanisterRuntime::new().add_times([200; 20]);
            process_pending_withdrawals(idle.clone()).await;
            assert!(idle.sent_update_calls().is_empty());
            read_state(|state| {
                assert_eq!(state.nonce_pool().num_free_accounts(), 0);
                assert!(state.pending_withdrawal_requests().contains_key(&43.into()));
            });
            let added_nonce = Address::from([101; 32]);
            record(EventType::Upgrade(UpgradeArgs {
                nonce_accounts_to_add: Some(vec![added_nonce.to_string()]),
                ..Default::default()
            }));
            let runtime = TestCanisterRuntime::new()
                .add_times([300; 20])
                .add_stub_response(nonce_read())
                .add_stub_response(sent())
                .add_signer(sign_as_minter());
            process_pending_withdrawals(runtime.clone()).await;
            assert!(halted());
            assert_eq!(runtime.sent_update_calls().len(), 2);
            read_state(|state| {
                assert!(state.sent_withdrawal_requests().contains_key(&43.into()));
                assert!(
                    state
                        .created_spl_withdrawal_txs()
                        .contains_key(&NONCE_ACCOUNT)
                );
                assert!(
                    !state
                        .created_spl_withdrawal_txs()
                        .contains_key(&added_nonce)
                );
                assert_eq!(state.nonce_pool().num_free_accounts(), 0);
            });
            assert_replay();
        }

        #[tokio::test]
        async fn should_not_count_held_spl_transactions_in_the_capacity_of_a_round() {
            let token = setup_funded(BALANCE, TOKENS);
            let held_nonce_accounts: Vec<Address> = (0..MAX_CONCURRENT_SIGNATURES as u8)
                .map(|index| Address::from([100 + index; 32]))
                .collect();
            record(EventType::Upgrade(UpgradeArgs {
                nonce_accounts_to_add: Some(
                    held_nonce_accounts
                        .iter()
                        .map(ToString::to_string)
                        .collect(),
                ),
                ..Default::default()
            }));
            for (id, nonce_account) in (0..).zip(&held_nonce_accounts) {
                queue_withdrawal(&token, id, 1_000);
                record(EventType::CreatedSplWithdrawalTransaction {
                    cksol_burn_block_index: id.into(),
                    nonce_account: *nonce_account,
                    nonce_value: durable_nonce(1),
                });
            }
            queue_sol_withdrawal(100, 1_000_000);
            halt();
            read_state(|state| {
                assert_eq!(
                    state.created_spl_withdrawal_txs().len(),
                    MAX_CONCURRENT_SIGNATURES
                );
                assert_eq!(state.nonce_pool().num_free_accounts(), 1);
                assert_eq!(state.num_unsigned_withdrawal_transactions(), 0);
            });
            let runtime = TestCanisterRuntime::new()
                .add_times([200; 20])
                .add_stub_response(nonce_read())
                .add_stub_response(sent())
                .add_signer(sign_as_minter());

            process_pending_withdrawals(runtime.clone()).await;

            // The free nonce account serves the SOL withdrawal.
            assert_eq!(
                runtime
                    .sent_update_calls()
                    .iter()
                    .map(|call| call.method.as_str())
                    .collect::<Vec<_>>(),
                ["getAccountInfo", "sendTransaction"]
            );
            read_state(|state| {
                assert!(state.sent_withdrawal_requests().contains_key(&100.into()));
                assert_eq!(
                    state.created_spl_withdrawal_txs().len(),
                    MAX_CONCURRENT_SIGNATURES
                );
            });

            set_halted(false);

            read_state(|state| {
                assert_eq!(
                    state.num_unsigned_withdrawal_transactions(),
                    MAX_CONCURRENT_SIGNATURES
                )
            });
            assert_replay();
        }
    }
}

mod settlement {
    use super::*;
    use crate::{
        constants::FEE_PER_SIGNATURE,
        monitor::{MIN_REBROADCAST_AGE, finalize_transactions},
        state::{
            SPL_SWEEP_TRANSACTION_FEE,
            event::{Signer, TransactionPurpose},
        },
        test_fixtures::{
            DEFAULT_BLOCK_HEIGHT, GetTransactionResult, devnet_sweep, durable_nonce,
            nonce_account_info, spl_sweep as fixture,
        },
    };
    use base64::{Engine, engine::general_purpose::STANDARD};
    use cksol_types::{SplRentRefundStatus, TxFinalizedStatus};
    use sol_rpc_types::MultiRpcResult;
    use solana_signature::Signature;
    use solana_transaction_status_client_types::EncodedConfirmedTransactionWithStatusMeta;

    type GetAccountInfoResult = MultiRpcResult<Option<sol_rpc_types::AccountInfo>>;

    const BALANCE: Lamport = 500_000_000;
    /// The treasury tokens after the credited sweep of the fixture deposit.
    const TOKENS: u64 = 1_000;
    const AMOUNT: u64 = 600;
    const TOKEN_FEE: u64 = 25;
    const TRANSFERRED: u64 = AMOUNT - TOKEN_FEE;
    const SOL_COST: Lamport = FEE_PER_SIGNATURE + RENT_RESERVE;
    const SENT_AT: u64 = 1_000;

    fn mint() -> Address {
        fixture::mint()
    }

    fn account_of_request() -> Account {
        account(1)
    }

    fn terms() -> SplWithdrawalRequestTerms {
        SplWithdrawalRequestTerms {
            account: account_of_request(),
            mint: mint(),
            ledger_id: fixture::registration().ledger_id,
            address: DESTINATION.parse().unwrap(),
            amount: AMOUNT,
            token_fee: TOKEN_FEE,
            fee: FEE,
            rent_reserve: RENT_RESERVE,
        }
    }

    fn record_at(time: u64, event: EventType) {
        mutate_state(|state| {
            process_event(
                state,
                event,
                &TestCanisterRuntime::new().add_times([time; 2]),
            )
        });
    }

    /// Funds the minter and the treasury, then accepts, binds and submits withdrawal 42
    /// with a signature of the minter key. Returns that signature.
    fn setup_sent_withdrawal() -> Signature {
        setup_sent_withdrawal_with_reserve(RENT_RESERVE)
    }

    fn setup_sent_withdrawal_with_reserve(rent_reserve: Lamport) -> Signature {
        reset_state();
        reset_events();
        init_state();
        fixture::init_balance(BALANCE + SPL_SWEEP_TRANSACTION_FEE);
        record_at(
            1,
            EventType::Upgrade(UpgradeArgs {
                add_spl_tokens: Some(vec![fixture::registration()]),
                ..Default::default()
            }),
        );
        record_at(
            1,
            EventType::RequestedOperatingRefill {
                refill_id: 0,
                amount: SPL_SWEEP_TRANSACTION_FEE,
                created_at_time: 1,
            },
        );
        record_at(
            1,
            EventType::CompletedOperatingRefill {
                refill_id: 0,
                burn_block_index: 7,
            },
        );
        let sweep = fixture::plan(0);
        let deposit = sweep.deposit();
        let sweep_signature = fixture::transaction(&sweep).signatures[0];
        record_at(
            1,
            EventType::QueuedSplDeposit {
                deposit_id: 0,
                account: deposit.account,
                mint: deposit.mint,
                address: deposit.address,
                balance: deposit.balance,
                fee: deposit.fee,
            },
        );
        record_at(
            1,
            EventType::SubmittedTransaction {
                signature: sweep_signature,
                message: sweep.sweep_message(Hash::new_from_array([0x42; 32])).into(),
                signers: sweep.signers().to_vec(),
                purpose: TransactionPurpose::SweepSplDeposit {
                    deposit_id: 0,
                    block_height: DEFAULT_BLOCK_HEIGHT,
                },
            },
        );
        record_at(
            1,
            EventType::SucceededTransaction {
                signature: sweep_signature,
            },
        );
        record_at(
            1,
            EventType::CreditedSplSweep {
                signature: sweep_signature,
                lamports_spent: SPL_SWEEP_TRANSACTION_FEE,
                deposit_id: 0,
                amount_to_mint: deposit.amount_to_mint(),
            },
        );
        read_state(|state| {
            assert_eq!(state.balance(), BALANCE);
            assert_eq!(state.operating_balance().balance(), 0);
            assert_eq!(state.spl_treasury_balance(&mint()), TOKENS);
        });

        record_at(
            1,
            EventType::AcceptedSplWithdrawalRequest(SplWithdrawalRequest {
                terms: SplWithdrawalRequestTerms {
                    rent_reserve,
                    ..terms()
                },
                cksol_burn_block_index: 42.into(),
                ckspl_burn_block_index: 7.into(),
            }),
        );
        record_at(
            1,
            EventType::CreatedSplWithdrawalTransaction {
                cksol_burn_block_index: 42.into(),
                nonce_account: NONCE_ACCOUNT,
                nonce_value: durable_nonce(1),
            },
        );
        let message = read_state(|state| {
            let created = state.created_spl_withdrawal_txs()[&NONCE_ACCOUNT].clone();
            state
                .bound_spl_withdrawal_message(&NONCE_ACCOUNT, &created)
                .unwrap()
        });
        let signature = devnet_sweep::minter_signature_of(&message);
        record_at(
            SENT_AT,
            EventType::SubmittedTransaction {
                signature,
                message: message.into(),
                signers: vec![Signer::Minter],
                purpose: TransactionPurpose::SplWithdrawal {
                    cksol_burn_block_index: 42.into(),
                },
            },
        );
        read_state(|state| {
            assert_eq!(state.balance(), BALANCE - FEE_PER_SIGNATURE - rent_reserve);
            assert_eq!(state.spl_treasury_balance(&mint()), TOKENS - TRANSFERRED);
        });
        signature
    }

    fn sent_message(signature: &Signature) -> solana_message::Message {
        read_state(
            |state| match state.submitted_transactions().get(signature) {
                Some(crate::state::MinterTransaction::SplWithdrawal {
                    message: crate::state::event::VersionedMessage::Legacy(message),
                    ..
                }) => message.clone(),
                other => panic!("expected a submitted SPL withdrawal, got {other:?}"),
            },
        )
    }

    fn advanced_nonce() -> GetAccountInfoResult {
        MultiRpcResult::Consistent(Ok(Some(nonce_account_info(
            devnet_sweep::minter_main_address(),
            3,
        ))))
    }

    fn unchanged_nonce() -> GetAccountInfoResult {
        MultiRpcResult::Consistent(Ok(Some(nonce_account_info(
            devnet_sweep::minter_main_address(),
            1,
        ))))
    }

    /// The `getTransaction` reply for the sent withdrawal. `rent` is the lamports that
    /// created the destination token account; `None` is a transaction that failed.
    fn outcome(
        signature: &Signature,
        rent: Option<Lamport>,
        with_token_balances: bool,
    ) -> GetTransactionResult {
        let message = sent_message(signature);
        let minter = devnet_sweep::minter_main_address();
        let index = |address: Address| {
            message
                .account_keys
                .iter()
                .position(|key| *key == address)
                .unwrap()
        };
        let treasury = index(associated_token_address(&minter, &mint()));
        let destination = index(associated_token_address(&terms().address, &mint()));
        let failed = rent.is_none();
        let rent = rent.unwrap_or(0);
        let mut pre = vec![1_000_000_u64; message.account_keys.len()];
        pre[0] = 10_000_000_000;
        pre[destination] = if rent == 0 && !failed {
            RENT_RESERVE
        } else {
            0
        };
        let mut post = pre.clone();
        post[0] -= FEE_PER_SIGNATURE + rent;
        post[destination] += rent;
        let token_balance = |index: usize, amount: u64| {
            let owner = if index == treasury {
                minter
            } else {
                terms().address
            };
            serde_json::json!({
                "accountIndex": index, "mint": mint().to_string(),
                "owner": owner.to_string(),
                "programId": spl_token_interface::id().to_string(),
                "uiTokenAmount": {"amount": amount.to_string(), "decimals": 6, "uiAmount": null, "uiAmountString": "0"}
            })
        };
        let (pre_tokens, post_tokens) = if !with_token_balances {
            (vec![], vec![])
        } else if failed {
            (
                vec![token_balance(treasury, TOKENS)],
                vec![token_balance(treasury, TOKENS)],
            )
        } else {
            // An existing destination token account has a balance before the transfer.
            let existing = if rent == 0 { Some(40) } else { None };
            (
                std::iter::once(token_balance(treasury, TOKENS))
                    .chain(existing.map(|amount| token_balance(destination, amount)))
                    .collect(),
                vec![
                    token_balance(treasury, TOKENS - TRANSFERRED),
                    token_balance(destination, existing.unwrap_or(0) + TRANSFERRED),
                ],
            )
        };
        let error = if failed {
            serde_json::json!({"InstructionError": [2, {"Custom": 17}]})
        } else {
            serde_json::Value::Null
        };
        let status = if failed {
            serde_json::json!({"Err": error.clone()})
        } else {
            serde_json::json!({"Ok": null})
        };
        let transaction = solana_transaction::Transaction {
            signatures: vec![*signature],
            message,
        };
        let encoded = STANDARD.encode(bincode::serialize(&transaction).unwrap());
        let outcome: EncodedConfirmedTransactionWithStatusMeta =
            serde_json::from_value(serde_json::json!({
                "slot": 350_000_000_u64,
                "blockTime": 1_700_000_000_i64,
                "meta": {
                    "err": error, "status": status, "fee": FEE_PER_SIGNATURE,
                    "preBalances": pre, "postBalances": post,
                    "innerInstructions": [], "logMessages": [],
                    "preTokenBalances": pre_tokens, "postTokenBalances": post_tokens,
                    "rewards": [], "loadedAddresses": {"readonly": [], "writable": []},
                    "computeUnitsConsumed": 450
                },
                "transaction": [encoded, "base64"],
                "version": "legacy"
            }))
            .unwrap();
        MultiRpcResult::Consistent(Ok(Some(outcome.try_into().unwrap())))
    }

    /// Runs the finalization timer with an advanced nonce and the given transaction.
    async fn finalize(reply: GetTransactionResult) -> TestCanisterRuntime {
        let runtime = TestCanisterRuntime::new()
            .add_times([2_000; 30])
            .add_stub_response(advanced_nonce())
            .add_stub_response(reply);
        finalize_transactions(runtime.clone()).await;
        runtime
    }

    /// Runs the finalization timer twice for a rent that halts SPL withdrawals: the first
    /// round records the halt, the second settles the withdrawal.
    async fn finalize_in_two_rounds(reply: GetTransactionResult) -> TestCanisterRuntime {
        finalize(reply.clone()).await;
        read_state(|state| assert!(state.spl_withdrawals_halted()));
        finalize(reply).await
    }

    /// Replays the recorded log with the signing key of this module as the minter key.
    fn assert_replay() {
        use crate::{
            state::{audit::replay_events, event::Event},
            test_fixtures::init_args_with_nonce_account,
        };
        use ic_stable_structures::Storable;
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
        let recorded = crate::storage::with_event_iter(|events| events.collect::<Vec<Event>>());
        let events = recorded.into_iter().map(|event| {
            let decoded = Event::from_bytes(std::borrow::Cow::Owned(event.to_bytes().into_owned()));
            let wire = candid::encode_one(cksol_types_internal::event::Event::from(event)).unwrap();
            let public: cksol_types_internal::event::Event = candid::decode_one(&wire).unwrap();
            assert_eq!(Event::try_from(public).unwrap(), decoded);
            decoded
        });
        let replayed = replay_events(prefix.chain(events));
        read_state(|state| assert_eq!(state, &replayed));
    }

    fn rent_refund_event(kind: SplRefundKind, amount: Lamport) -> EventType {
        EventType::QueuedSplCkSolRefund {
            cksol_burn_block_index: 42.into(),
            kind,
            to: account_of_request(),
            amount,
            created_at_time: 2_000,
        }
    }

    fn assert_no_refund_recorded() {
        let recorded = crate::storage::with_event_iter(|events| events.collect::<Vec<_>>());
        assert!(
            !recorded
                .iter()
                .any(|event| matches!(event.payload, EventType::QueuedSplCkSolRefund { .. }))
        );
    }

    #[tokio::test]
    async fn should_settle_a_transfer_that_created_the_destination_account() {
        let signature = setup_sent_withdrawal();
        let events_before = EventsAssert::from_recorded();

        let runtime = finalize(outcome(&signature, Some(RENT_RESERVE), true)).await;

        assert_eq!(
            runtime
                .sent_update_calls()
                .iter()
                .map(|call| call.method.as_str())
                .collect::<Vec<_>>(),
            ["getAccountInfo", "getTransaction"]
        );
        // The whole reserve was spent, so the outcome is the only event.
        assert_eq!(EventsAssert::from_recorded().len(), events_before.len() + 1);
        EventsAssert::from_recorded().expect_contains_event_eq(
            EventType::SucceededSplWithdrawalTransaction {
                signature,
                lamports_spent: SOL_COST,
                rent_spent: RENT_RESERVE,
            },
        );
        assert_no_refund_recorded();
        read_state(|state| {
            assert!(state.submitted_transactions().is_empty());
            assert!(state.succeeded_transactions().contains(&signature));
            assert!(state.sent_spl_withdrawal_requests().is_empty());
            assert_eq!(state.nonce_pool().num_free_accounts(), 1);
            // The reservation was spent in full; the fee less the transaction fee is
            // the minter's own SOL.
            assert_eq!(state.balance(), BALANCE - SOL_COST);
            assert_eq!(state.operating_balance().balance(), FEE - FEE_PER_SIGNATURE);
            assert_eq!(state.spl_treasury_balance(&mint()), TOKENS - TRANSFERRED);
            assert_eq!(
                state.collected_spl_withdrawal_fees(&mint()),
                u128::from(TOKEN_FEE)
            );
            assert_eq!(
                state.all_collected_spl_withdrawal_fees(),
                [cksol_types::CollectedSplWithdrawalFees {
                    mint: mint().into(),
                    amount: u128::from(TOKEN_FEE),
                }]
            );
        });
        assert_eq!(
            withdrawal_status(42),
            WithdrawSplStatus::TxFinalized {
                transaction: TxFinalizedStatus::Success {
                    transaction_id: signature.into()
                },
                rent_refund: SplRentRefundStatus::NotDue,
            }
        );
        assert_replay();
        // A settled withdrawal is not read again.
        let idle = TestCanisterRuntime::new().add_times([3_000; 30]);
        finalize_transactions(idle.clone()).await;
        assert!(idle.sent_update_calls().is_empty());
    }

    #[tokio::test]
    async fn should_refund_the_rent_reserve_when_the_destination_account_existed() {
        let signature = setup_sent_withdrawal();

        finalize(outcome(&signature, Some(0), true)).await;

        let recorded = crate::storage::with_event_iter(|events| events.collect::<Vec<_>>());
        assert_eq!(
            recorded[recorded.len() - 2..]
                .iter()
                .map(|event| event.payload.clone())
                .collect::<Vec<_>>(),
            [
                EventType::SucceededSplWithdrawalTransaction {
                    signature,
                    lamports_spent: FEE_PER_SIGNATURE,
                    rent_spent: 0,
                },
                rent_refund_event(SplRefundKind::UnusedRent, RENT_RESERVE),
            ]
        );
        read_state(|state| {
            // The rent returns to the balance: it backs the ckSOL that the refund mints.
            assert_eq!(state.balance(), BALANCE - FEE_PER_SIGNATURE);
            assert_eq!(state.operating_balance().balance(), FEE - FEE_PER_SIGNATURE);
        });
        let finalized = |rent_refund| WithdrawSplStatus::TxFinalized {
            transaction: TxFinalizedStatus::Success {
                transaction_id: signature.into(),
            },
            rent_refund,
        };
        assert_eq!(
            withdrawal_status(42),
            finalized(SplRentRefundStatus::Pending {
                amount: RENT_RESERVE
            })
        );
        assert_replay();

        let ledger = reimbursement_runtime([Ok(Nat::from(8_u64))]);
        process_pending_cksol_refunds(ledger.clone()).await;

        let calls = ledger.sent_update_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].canister_id, ledger_canister_id());
        assert_eq!(
            calls[0].single_arg::<TransferArg>(),
            TransferArg {
                from_subaccount: None,
                to: account_of_request(),
                fee: None,
                created_at_time: Some(2_000),
                memo: Some(
                    Memo::from(MintMemo::spl_cksol_refund(42, SplRefundKind::UnusedRent)).into()
                ),
                amount: RENT_RESERVE.into(),
            }
        );
        assert_eq!(
            withdrawal_status(42),
            finalized(SplRentRefundStatus::Reimbursed {
                amount: RENT_RESERVE,
                mint_block_index: 8,
            })
        );
        assert_replay();
    }

    #[tokio::test]
    async fn should_complete_a_transfer_whose_rent_is_above_the_reserve() {
        let signature = setup_sent_withdrawal();
        let excess = 160_720;

        finalize_in_two_rounds(outcome(&signature, Some(RENT_RESERVE + excess), true)).await;

        EventsAssert::from_recorded().expect_contains_event_eq(
            EventType::SucceededSplWithdrawalTransaction {
                signature,
                lamports_spent: SOL_COST + excess,
                rent_spent: RENT_RESERVE + excess,
            },
        );
        assert_no_refund_recorded();
        read_state(|state| {
            // The fee absorbs the rent above the reserve.
            assert_eq!(state.balance(), BALANCE - SOL_COST - excess);
            assert_eq!(
                state.operating_balance().balance(),
                FEE - FEE_PER_SIGNATURE - excess
            );
        });
        assert!(matches!(
            withdrawal_status(42),
            WithdrawSplStatus::TxFinalized {
                rent_refund: SplRentRefundStatus::NotDue,
                ..
            }
        ));
        assert_replay();
    }

    #[tokio::test]
    async fn should_take_a_rent_overrun_above_the_fee_from_the_operating_balance() {
        // The fee less the transaction fee absorbs this much rent above the reserve.
        let margin = FEE - FEE_PER_SIGNATURE;
        for (operating, expected_operating, expected_shortfall) in
            [(10, 9, 0), (1, 0, 0), (0, 0, 1)]
        {
            let signature = setup_sent_withdrawal();
            if operating > 0 {
                record_at(
                    1,
                    EventType::RequestedOperatingRefill {
                        refill_id: 1,
                        amount: operating,
                        created_at_time: 1,
                    },
                );
                record_at(
                    1,
                    EventType::CompletedOperatingRefill {
                        refill_id: 1,
                        burn_block_index: 9,
                    },
                );
            }
            let rent = RENT_RESERVE + margin + 1;

            finalize_in_two_rounds(outcome(&signature, Some(rent), true)).await;

            read_state(|state| {
                // The main balance pays the real cost in every case.
                assert_eq!(
                    state.balance(),
                    BALANCE - FEE_PER_SIGNATURE - rent,
                    "operating {operating}"
                );
                // The one lamport above the fee is booked: it comes from the operating
                // balance, and from the shortfall counter when that balance is empty.
                assert_eq!(
                    state.operating_balance().balance(),
                    expected_operating,
                    "operating {operating}"
                );
                assert_eq!(
                    state.operating_balance().shortfall(),
                    expected_shortfall,
                    "operating {operating}"
                );
            });
            assert!(matches!(
                withdrawal_status(42),
                WithdrawSplStatus::TxFinalized {
                    rent_refund: SplRentRefundStatus::NotDue,
                    ..
                }
            ));
            assert_replay();
        }
    }

    #[tokio::test]
    async fn should_repair_a_shortfall_from_new_funding_before_it_is_spendable() {
        let signature = setup_sent_withdrawal();
        let margin = FEE - FEE_PER_SIGNATURE;
        finalize_in_two_rounds(outcome(&signature, Some(RENT_RESERVE + margin + 1), true)).await;
        read_state(|state| {
            assert_eq!(state.operating_balance().balance(), 0);
            assert_eq!(state.operating_balance().shortfall(), 1);
            assert_eq!(state.spl_sweep_allowance(), 0);
        });

        assert!(halted());

        // A refill of 10 lamports repairs the lamport that is owed and leaves 9.
        record_at(
            3_000,
            EventType::RequestedOperatingRefill {
                refill_id: 1,
                amount: 10,
                created_at_time: 3_000,
            },
        );
        record_at(
            3_000,
            EventType::CompletedOperatingRefill {
                refill_id: 1,
                burn_block_index: 9,
            },
        );

        read_state(|state| {
            assert_eq!(state.operating_balance().balance(), 9);
            assert_eq!(state.operating_balance().shortfall(), 0);
            assert_eq!(state.spl_sweep_allowance(), 9);
        });
        assert!(halted(), "a refill must not resume withdrawals");
        assert_replay();
    }

    #[tokio::test]
    async fn should_repair_a_shortfall_from_a_released_sweep_reservation() {
        let signature = setup_sent_withdrawal();
        // A second sweep is in flight, so its fee is reserved from the operating balance.
        let sweep = fixture::plan(1);
        let deposit = sweep.deposit();
        let sweep_signature = fixture::transaction(&sweep).signatures[0];
        record_at(
            1,
            EventType::RequestedOperatingRefill {
                refill_id: 1,
                amount: SPL_SWEEP_TRANSACTION_FEE,
                created_at_time: 1,
            },
        );
        record_at(
            1,
            EventType::CompletedOperatingRefill {
                refill_id: 1,
                burn_block_index: 8,
            },
        );
        record_at(
            1,
            EventType::QueuedSplDeposit {
                deposit_id: 1,
                account: deposit.account,
                mint: deposit.mint,
                address: deposit.address,
                balance: deposit.balance,
                fee: deposit.fee,
            },
        );
        record_at(
            1,
            EventType::SubmittedTransaction {
                signature: sweep_signature,
                message: sweep.sweep_message(Hash::new_from_array([0x42; 32])).into(),
                signers: sweep.signers().to_vec(),
                purpose: TransactionPurpose::SweepSplDeposit {
                    deposit_id: 1,
                    block_height: DEFAULT_BLOCK_HEIGHT,
                },
            },
        );
        read_state(|state| assert_eq!(state.operating_balance().balance(), 0));

        // The reserved SOL cannot pay the overrun, so one lamport becomes a shortfall.
        let margin = FEE - FEE_PER_SIGNATURE;
        let rent = RENT_RESERVE + margin + 1;
        record_at(
            2_000,
            EventType::SucceededSplWithdrawalTransaction {
                signature,
                lamports_spent: FEE_PER_SIGNATURE + rent,
                rent_spent: rent,
            },
        );
        read_state(|state| {
            assert_eq!(state.operating_balance().balance(), 0);
            assert_eq!(state.operating_balance().shortfall(), 1);
        });

        // The sweep expires and its reservation returns: it repairs the shortfall first.
        record_at(
            3_000,
            EventType::ExpiredTransaction {
                signature: sweep_signature,
            },
        );

        read_state(|state| {
            assert_eq!(state.operating_balance().shortfall(), 0);
            assert_eq!(
                state.operating_balance().balance(),
                SPL_SWEEP_TRANSACTION_FEE - 1
            );
            assert_eq!(state.spl_sweep_allowance(), SPL_SWEEP_TRANSACTION_FEE - 1);
        });
        assert_replay();
    }

    #[tokio::test]
    async fn should_repair_a_shortfall_from_the_fee_of_a_later_withdrawal() {
        let signature = setup_sent_withdrawal();
        let margin = FEE - FEE_PER_SIGNATURE;
        finalize_in_two_rounds(outcome(&signature, Some(RENT_RESERVE + margin + 1), true)).await;
        read_state(|state| assert_eq!(state.operating_balance().shortfall(), 1));

        // A second withdrawal fails on-chain, so its fee less the transaction fee is kept.
        record_at(
            3_000,
            EventType::AcceptedSplWithdrawalRequest(SplWithdrawalRequest {
                terms: SplWithdrawalRequestTerms {
                    amount: TOKEN_FEE + 1,
                    ..terms()
                },
                cksol_burn_block_index: 43.into(),
                ckspl_burn_block_index: 8.into(),
            }),
        );
        record_at(
            3_000,
            EventType::CreatedSplWithdrawalTransaction {
                cksol_burn_block_index: 43.into(),
                nonce_account: NONCE_ACCOUNT,
                nonce_value: durable_nonce(4),
            },
        );
        let message = read_state(|state| {
            let created = state.created_spl_withdrawal_txs()[&NONCE_ACCOUNT].clone();
            state
                .bound_spl_withdrawal_message(&NONCE_ACCOUNT, &created)
                .unwrap()
        });
        let second = devnet_sweep::minter_signature_of(&message);
        record_at(
            3_000,
            EventType::SubmittedTransaction {
                signature: second,
                message: message.into(),
                signers: vec![Signer::Minter],
                purpose: TransactionPurpose::SplWithdrawal {
                    cksol_burn_block_index: 43.into(),
                },
            },
        );
        record_at(
            4_000,
            EventType::FailedSplWithdrawalTransaction {
                signature: second,
                lamports_spent: FEE_PER_SIGNATURE,
            },
        );

        read_state(|state| {
            assert_eq!(state.operating_balance().shortfall(), 0);
            assert_eq!(state.operating_balance().balance(), margin - 1);
        });
        assert_replay();
    }

    #[tokio::test]
    async fn should_credit_nothing_when_the_fee_equals_the_cost_of_the_withdrawal() {
        let signature = setup_sent_withdrawal();
        let margin = FEE - FEE_PER_SIGNATURE;

        finalize_in_two_rounds(outcome(&signature, Some(RENT_RESERVE + margin), true)).await;

        read_state(|state| {
            assert_eq!(state.operating_balance().balance(), 0);
            assert_eq!(state.operating_balance().shortfall(), 0);
        });
        assert_replay();
    }

    #[tokio::test]
    async fn should_remint_the_tokens_and_refund_the_rent_after_a_failed_transfer() {
        let signature = setup_sent_withdrawal();

        finalize(outcome(&signature, None, true)).await;

        let recorded = crate::storage::with_event_iter(|events| events.collect::<Vec<_>>());
        assert_eq!(
            recorded[recorded.len() - 2..]
                .iter()
                .map(|event| event.payload.clone())
                .collect::<Vec<_>>(),
            [
                EventType::FailedSplWithdrawalTransaction {
                    signature,
                    lamports_spent: FEE_PER_SIGNATURE,
                },
                rent_refund_event(SplRefundKind::FailedTransferRent, RENT_RESERVE),
            ]
        );
        read_state(|state| {
            assert!(state.submitted_transactions().is_empty());
            assert!(state.failed_transactions().contains_key(&signature));
            assert_eq!(state.nonce_pool().num_free_accounts(), 1);
            // The tokens return to the treasury and the rent to the balance. The fee is
            // kept and pays the failed transaction.
            assert_eq!(state.spl_treasury_balance(&mint()), TOKENS);
            assert_eq!(state.balance(), BALANCE - FEE_PER_SIGNATURE);
            assert_eq!(state.operating_balance().balance(), FEE - FEE_PER_SIGNATURE);
            assert_eq!(state.collected_spl_withdrawal_fees(&mint()), 0);
        });
        let pending_rent = SplRentRefundStatus::Pending {
            amount: RENT_RESERVE,
        };
        assert_eq!(
            withdrawal_status(42),
            WithdrawSplStatus::ReimbursementPending {
                transaction_id: signature.into(),
                reimbursed_amount: AMOUNT,
                rent_refund: pending_rent.clone(),
            }
        );
        assert_replay();

        // The remint is the full burned amount, on the ledger of the token.
        let token_ledger = reimbursement_runtime([Ok(Nat::from(5_u64))]);
        process_pending_reimbursements(token_ledger.clone()).await;
        let calls = token_ledger.sent_update_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].canister_id, fixture::registration().ledger_id);
        assert_eq!(
            calls[0].single_arg::<TransferArg>(),
            TransferArg {
                from_subaccount: None,
                to: account_of_request(),
                fee: None,
                created_at_time: Some(2_000),
                memo: Some(Memo::from(MintMemo::spl_withdrawal_reimbursement(42)).into()),
                amount: AMOUNT.into(),
            }
        );
        assert_eq!(
            withdrawal_status(42),
            WithdrawSplStatus::Reimbursed {
                transaction_id: signature.into(),
                reimbursed_amount: AMOUNT,
                mint_block_index: 5,
                rent_refund: pending_rent,
            }
        );

        // The rent refund is an independent obligation on the ckSOL ledger.
        let cksol_ledger = reimbursement_runtime([Ok(Nat::from(8_u64))]);
        process_pending_cksol_refunds(cksol_ledger.clone()).await;
        assert_eq!(
            cksol_ledger.sent_update_calls()[0]
                .single_arg::<TransferArg>()
                .memo,
            Some(
                Memo::from(MintMemo::spl_cksol_refund(
                    42,
                    SplRefundKind::FailedTransferRent
                ))
                .into()
            )
        );
        assert_eq!(
            withdrawal_status(42),
            WithdrawSplStatus::Reimbursed {
                transaction_id: signature.into(),
                reimbursed_amount: AMOUNT,
                mint_block_index: 5,
                rent_refund: SplRentRefundStatus::Reimbursed {
                    amount: RENT_RESERVE,
                    mint_block_index: 8,
                },
            }
        );
        assert_replay();
    }

    #[tokio::test]
    async fn should_quarantine_a_token_remint_that_cannot_be_retried_safely() {
        let signature = setup_sent_withdrawal();
        finalize(outcome(&signature, None, true)).await;

        let token_ledger = reimbursement_runtime([Err(TransferError::TooOld)]);
        process_pending_reimbursements(token_ledger).await;

        EventsAssert::from_recorded().expect_contains_event_eq(
            EventType::QuarantinedSplWithdrawalReimbursement {
                cksol_burn_block_index: 42.into(),
            },
        );
        assert!(matches!(
            withdrawal_status(42),
            WithdrawSplStatus::ReimbursementQuarantined {
                reimbursed_amount: AMOUNT,
                ..
            }
        ));
        // A quarantined remint is not sent again.
        let idle = reimbursement_runtime([]);
        process_pending_reimbursements(idle.clone()).await;
        assert!(idle.sent_update_calls().is_empty());
        assert_replay();
    }

    #[tokio::test]
    async fn should_keep_the_withdrawal_in_flight_until_its_metadata_is_complete() {
        let signature = setup_sent_withdrawal();
        let events_before = EventsAssert::from_recorded();

        // The reply has no token balances, so the transfer is not proven.
        finalize(outcome(&signature, Some(RENT_RESERVE), false)).await;

        assert_eq!(EventsAssert::from_recorded(), events_before);
        assert_eq!(
            withdrawal_status(42),
            WithdrawSplStatus::TxSent {
                transaction_id: signature.into()
            }
        );
        read_state(|state| {
            assert_eq!(state.balance(), BALANCE - SOL_COST);
            assert_eq!(state.nonce_pool().num_free_accounts(), 0);
        });

        // The next round reads the transaction again and settles it once.
        finalize(outcome(&signature, Some(RENT_RESERVE), true)).await;
        assert_eq!(EventsAssert::from_recorded().len(), events_before.len() + 1);
        assert!(matches!(
            withdrawal_status(42),
            WithdrawSplStatus::TxFinalized { .. }
        ));
    }

    #[tokio::test]
    async fn should_rebroadcast_an_spl_withdrawal_whose_nonce_did_not_advance() {
        let signature = setup_sent_withdrawal();
        let events_before = EventsAssert::from_recorded();
        let old_enough = SENT_AT + MIN_REBROADCAST_AGE.as_nanos() as u64;
        let runtime = TestCanisterRuntime::new()
            .add_times([old_enough; 30])
            .add_stub_response(unchanged_nonce())
            .add_stub_response(MultiRpcResult::<sol_rpc_types::Signature>::Consistent(Ok(
                signature.into(),
            )));

        finalize_transactions(runtime.clone()).await;

        assert_eq!(
            runtime
                .sent_update_calls()
                .iter()
                .map(|call| call.method.as_str())
                .collect::<Vec<_>>(),
            ["getAccountInfo", "sendTransaction"]
        );
        assert_eq!(EventsAssert::from_recorded(), events_before);
        assert!(matches!(
            withdrawal_status(42),
            WithdrawSplStatus::TxSent { .. }
        ));
    }

    #[tokio::test]
    #[should_panic(expected = "another amount than the unused rent")]
    async fn should_reject_a_rent_refund_of_another_amount_than_the_unused_rent() {
        let signature = setup_sent_withdrawal();
        record_at(
            2_000,
            EventType::SucceededSplWithdrawalTransaction {
                signature,
                lamports_spent: FEE_PER_SIGNATURE + 39_280,
                rent_spent: 39_280,
            },
        );
        record_at(
            2_000,
            rent_refund_event(SplRefundKind::UnusedRent, RENT_RESERVE),
        );
    }

    #[tokio::test]
    #[should_panic(expected = "another amount than the rent reserve")]
    async fn should_reject_a_partial_rent_refund_after_a_failed_transfer() {
        let signature = setup_sent_withdrawal();
        record_at(
            2_000,
            EventType::FailedSplWithdrawalTransaction {
                signature,
                lamports_spent: FEE_PER_SIGNATURE,
            },
        );
        record_at(
            2_000,
            rent_refund_event(SplRefundKind::FailedTransferRent, RENT_RESERVE - 1),
        );
    }

    #[test]
    #[should_panic(expected = "that is not an SPL withdrawal")]
    fn should_reject_the_spl_outcome_event_for_another_transaction() {
        setup_sent_withdrawal();
        let sweep_signature = fixture::transaction(&fixture::plan(1)).signatures[0];
        let sweep = fixture::plan(1);
        let deposit = sweep.deposit();
        record_at(
            1,
            EventType::RequestedOperatingRefill {
                refill_id: 1,
                amount: SPL_SWEEP_TRANSACTION_FEE,
                created_at_time: 1,
            },
        );
        record_at(
            1,
            EventType::CompletedOperatingRefill {
                refill_id: 1,
                burn_block_index: 8,
            },
        );
        record_at(
            1,
            EventType::QueuedSplDeposit {
                deposit_id: 1,
                account: deposit.account,
                mint: deposit.mint,
                address: deposit.address,
                balance: deposit.balance,
                fee: deposit.fee,
            },
        );
        record_at(
            1,
            EventType::SubmittedTransaction {
                signature: sweep_signature,
                message: sweep.sweep_message(Hash::new_from_array([0x42; 32])).into(),
                signers: sweep.signers().to_vec(),
                purpose: TransactionPurpose::SweepSplDeposit {
                    deposit_id: 1,
                    block_height: DEFAULT_BLOCK_HEIGHT,
                },
            },
        );
        record_at(
            2_000,
            EventType::SucceededSplWithdrawalTransaction {
                signature: sweep_signature,
                lamports_spent: FEE_PER_SIGNATURE,
                rent_spent: 0,
            },
        );
    }

    #[test]
    fn should_preserve_the_tags_of_the_outcome_and_remint_events() {
        let signature = Signature::from([7; 64]);
        for (payload, expected) in [
            (
                EventType::SucceededSplWithdrawalTransaction {
                    signature,
                    lamports_spent: 2,
                    rent_spent: 1,
                },
                28,
            ),
            (
                EventType::FailedSplWithdrawalTransaction {
                    signature,
                    lamports_spent: 1,
                },
                29,
            ),
            (
                EventType::ReimbursedSplWithdrawal {
                    cksol_burn_block_index: 42.into(),
                    mint_block_index: 5.into(),
                },
                33,
            ),
            (
                EventType::QuarantinedSplWithdrawalReimbursement {
                    cksol_burn_block_index: 42.into(),
                },
                34,
            ),
            (
                EventType::HaltedSplWithdrawals {
                    signature,
                    rent_spent: 2,
                    rent_reserve: 1,
                },
                36,
            ),
        ] {
            let encoded = minicbor::to_vec(payload).unwrap();
            let mut decoder = minicbor::Decoder::new(&encoded);
            assert_eq!(decoder.array().unwrap(), Some(2));
            assert_eq!(decoder.u32().unwrap(), expected);
        }
    }

    mod halt {
        use super::*;
        use crate::test_fixtures::assert_spl_withdrawal_metrics;

        fn halt_event(signature: Signature, rent_spent: Lamport) -> EventType {
            EventType::HaltedSplWithdrawals {
                signature,
                rent_spent,
                rent_reserve: RENT_RESERVE,
            }
        }

        fn recorded_halts() -> usize {
            crate::storage::with_event_iter(|events| {
                events
                    .filter(|event| matches!(event.payload, EventType::HaltedSplWithdrawals { .. }))
                    .count()
            })
        }

        #[tokio::test]
        async fn should_halt_when_the_rent_is_above_the_reserve() {
            let signature = setup_sent_withdrawal();
            assert_spl_withdrawal_metrics(0, &["spl_withdrawals_halted 0 "]);

            let events_before = EventsAssert::from_recorded();

            finalize(outcome(&signature, Some(RENT_RESERVE + 1), true)).await;

            // Round 1 records only the halt. The withdrawal is not settled yet.
            let events_after = EventsAssert::from_recorded();
            assert_eq!(events_after.len(), events_before.len() + 1);
            events_after.expect_contains_event_eq(halt_event(signature, RENT_RESERVE + 1));
            assert!(halted());
            assert!(matches!(
                withdrawal_status(42),
                WithdrawSplStatus::TxSent { .. }
            ));
            assert_spl_withdrawal_metrics(0, &["spl_withdrawals_halted 1 "]);
            assert_replay();

            finalize(outcome(&signature, Some(RENT_RESERVE + 1), true)).await;

            // Round 2 settles the withdrawal. There is still one halt event.
            EventsAssert::from_recorded().expect_contains_event_eq(
                EventType::SucceededSplWithdrawalTransaction {
                    signature,
                    lamports_spent: SOL_COST + 1,
                    rent_spent: RENT_RESERVE + 1,
                },
            );
            assert_eq!(recorded_halts(), 1);
            assert!(halted());
            assert!(matches!(
                withdrawal_status(42),
                WithdrawSplStatus::TxFinalized { .. }
            ));
            assert_replay();
        }

        #[tokio::test]
        async fn should_not_halt_when_the_rent_is_within_the_reserve_or_the_transfer_failed() {
            for rent in [Some(0), Some(RENT_RESERVE), None] {
                let signature = setup_sent_withdrawal();

                finalize(outcome(&signature, rent, true)).await;

                read_state(|state| assert!(state.sent_spl_withdrawal_requests().is_empty()));
                assert_eq!(recorded_halts(), 0, "rent {rent:?}");
                assert!(!halted(), "rent {rent:?}");
                assert_spl_withdrawal_metrics(0, &["spl_withdrawals_halted 0 "]);
            }
        }

        #[tokio::test]
        async fn should_compare_the_rent_of_an_older_request_with_the_current_reserve() {
            // The reserve of the request is lower than the current reserve. This happens
            // when an upgrade raises the reserve while the request waits.
            let captured = RENT_RESERVE - 100_000;
            for (rent, halts) in [(RENT_RESERVE, 0), (RENT_RESERVE + 1, 1)] {
                let signature = setup_sent_withdrawal_with_reserve(captured);

                if halts == 1 {
                    finalize_in_two_rounds(outcome(&signature, Some(rent), true)).await;
                } else {
                    finalize(outcome(&signature, Some(rent), true)).await;
                }

                read_state(|state| {
                    assert_eq!(state.spl_withdrawals_above_rent_reserve(), 1);
                    assert_eq!(state.spl_rent_above_reserve(), rent - captured);
                    assert_eq!(state.spl_withdrawals_halted(), halts == 1);
                });
                assert_eq!(recorded_halts(), halts);
                if halts == 1 {
                    EventsAssert::from_recorded()
                        .expect_contains_event_eq(halt_event(signature, rent));
                }
                assert_replay();
            }
        }

        #[tokio::test]
        async fn should_not_halt_for_a_rent_within_a_captured_reserve_above_the_current_one() {
            // The reserve of the request is higher than the current reserve, and the rent
            // is within it.
            let captured = RENT_RESERVE + 100_000;
            let signature = setup_sent_withdrawal_with_reserve(captured);

            finalize(outcome(&signature, Some(RENT_RESERVE + 1), true)).await;

            read_state(|state| {
                assert!(state.sent_spl_withdrawal_requests().is_empty());
                assert_eq!(state.spl_withdrawals_above_rent_reserve(), 0);
            });
            assert_eq!(recorded_halts(), 0);
            assert!(!halted());
            assert_replay();
        }

        #[tokio::test]
        async fn should_settle_and_refund_sent_withdrawals_while_halted() {
            for (rent, refund) in [
                (Some(0), Some(SplRefundKind::UnusedRent)),
                (None, Some(SplRefundKind::FailedTransferRent)),
                (Some(RENT_RESERVE + 1), None),
            ] {
                let signature = setup_sent_withdrawal();
                halt();

                finalize(outcome(&signature, rent, true)).await;

                read_state(|state| assert!(state.sent_spl_withdrawal_requests().is_empty()));
                match refund {
                    Some(kind) => {
                        EventsAssert::from_recorded()
                            .expect_contains_event_eq(rent_refund_event(kind, RENT_RESERVE));
                    }
                    None => assert_no_refund_recorded(),
                }
                // The halt is recorded once.
                assert_eq!(recorded_halts(), 1, "rent {rent:?}");
                assert!(halted());
                assert_replay();

                if rent.is_none() {
                    let token_ledger = reimbursement_runtime([Ok(Nat::from(5_u64))]);
                    process_pending_reimbursements(token_ledger.clone()).await;
                    let calls = token_ledger.sent_update_calls();
                    assert_eq!(calls.len(), 1);
                    assert_eq!(calls[0].canister_id, fixture::registration().ledger_id);
                    assert_eq!(
                        calls[0].single_arg::<TransferArg>().amount,
                        Nat::from(AMOUNT)
                    );
                }
                if refund.is_some() {
                    let fee_ledger = reimbursement_runtime([Ok(Nat::from(8_u64))]);
                    process_pending_cksol_refunds(fee_ledger.clone()).await;
                    let calls = fee_ledger.sent_update_calls();
                    assert_eq!(calls.len(), 1);
                    assert_eq!(calls[0].canister_id, ledger_canister_id());
                    assert_eq!(
                        calls[0].single_arg::<TransferArg>().amount,
                        Nat::from(RENT_RESERVE)
                    );
                    assert_eq!(
                        calls[0].single_arg::<TransferArg>().to,
                        account_of_request()
                    );
                    let expected_rent = SplRentRefundStatus::Reimbursed {
                        amount: RENT_RESERVE,
                        mint_block_index: 8,
                    };
                    match withdrawal_status(42) {
                        WithdrawSplStatus::Reimbursed {
                            mint_block_index,
                            rent_refund,
                            ..
                        } => {
                            assert!(rent.is_none());
                            assert_eq!(mint_block_index, 5);
                            assert_eq!(rent_refund, expected_rent);
                        }
                        WithdrawSplStatus::TxFinalized { rent_refund, .. } => {
                            assert!(rent.is_some());
                            assert_eq!(rent_refund, expected_rent);
                        }
                        other => panic!("refund did not complete while halted: {other:?}"),
                    }
                }
                // A second run of the two refund timers sends no ledger call.
                let idle = reimbursement_runtime([]);
                process_pending_reimbursements(idle.clone()).await;
                process_pending_cksol_refunds(idle.clone()).await;
                assert!(idle.sent_update_calls().is_empty());
                assert!(halted());
                assert_replay();
            }
        }

        #[test]
        fn should_not_decide_the_halt_again_on_replay() {
            let signature = setup_sent_withdrawal();

            record_at(
                2_000,
                EventType::SucceededSplWithdrawalTransaction {
                    signature,
                    lamports_spent: SOL_COST + 1,
                    rent_spent: RENT_RESERVE + 1,
                },
            );

            read_state(|state| assert_eq!(state.spl_withdrawals_above_rent_reserve(), 1));
            assert!(!halted());
            assert_replay();
        }
    }

    mod metrics {
        use super::*;
        use crate::test_fixtures::assert_spl_withdrawal_metrics;

        const SECOND: u64 = 1_000_000_000;

        fn failed_withdrawals(frozen_account: u64) -> [String; 3] {
            let mint = mint();
            [
                format!(
                    r#"failed_spl_withdrawals{{mint="{mint}",reason="frozen_account"}} {frozen_account} "#
                ),
                format!(r#"failed_spl_withdrawals{{mint="{mint}",reason="missing_account"}} 0 "#),
                format!(r#"failed_spl_withdrawals{{mint="{mint}",reason="other"}} 0 "#),
            ]
        }

        fn assert_metrics(now: u64, expected: &[String]) {
            let expected: Vec<&str> = expected.iter().map(String::as_str).collect();
            assert_spl_withdrawal_metrics(now, &expected);
        }

        #[tokio::test]
        async fn should_count_a_failed_withdrawal_once_across_observation_rounds() {
            let signature = setup_sent_withdrawal();

            // The transaction is not readable yet, so the withdrawal stays in flight.
            finalize(MultiRpcResult::Consistent(Ok(None))).await;
            assert_metrics(0, &failed_withdrawals(0));
            assert!(matches!(
                withdrawal_status(42),
                WithdrawSplStatus::TxSent { .. }
            ));

            finalize(outcome(&signature, None, true)).await;
            assert_metrics(0, &failed_withdrawals(1));

            // No transaction is in flight, so a later round reads and counts nothing.
            let idle = TestCanisterRuntime::new().add_times([3_000; 30]);
            finalize_transactions(idle.clone()).await;
            assert!(idle.sent_update_calls().is_empty());
            assert_metrics(0, &failed_withdrawals(1));
        }

        #[tokio::test]
        async fn should_keep_the_failure_totals_of_two_mints_independent() {
            use crate::storage::{FailedSplTransferReason, record_failed_spl_withdrawal};

            let signature = setup_sent_withdrawal();
            let other_mint: Address = [7; 32].into();
            record_at(
                1,
                EventType::Upgrade(UpgradeArgs {
                    add_spl_tokens: Some(vec![SplTokenRegistration {
                        mint: other_mint.to_string(),
                        ledger_id: Principal::from_slice(&[99, 2]),
                        symbol: "ckOther".into(),
                        ..fixture::registration()
                    }]),
                    ..Default::default()
                }),
            );
            let totals = |mint: Address, [frozen_account, missing_account, other]: [u64; 3]| {
                [
                    format!(
                        r#"failed_spl_withdrawals{{mint="{mint}",reason="frozen_account"}} {frozen_account} "#
                    ),
                    format!(
                        r#"failed_spl_withdrawals{{mint="{mint}",reason="missing_account"}} {missing_account} "#
                    ),
                    format!(r#"failed_spl_withdrawals{{mint="{mint}",reason="other"}} {other} "#),
                ]
            };

            // The monitor counts the failed withdrawal for the mint of its request only.
            finalize(outcome(&signature, None, true)).await;
            assert_metrics(0, &totals(mint(), [1, 0, 0]));
            assert_metrics(0, &totals(other_mint, [0, 0, 0]));

            // Failures of the other mint do not change the totals of the first mint.
            record_failed_spl_withdrawal(other_mint, FailedSplTransferReason::Other);
            record_failed_spl_withdrawal(other_mint, FailedSplTransferReason::Other);
            record_failed_spl_withdrawal(other_mint, FailedSplTransferReason::MissingAccount);
            assert_metrics(0, &totals(mint(), [1, 0, 0]));
            assert_metrics(0, &totals(other_mint, [0, 1, 2]));
        }

        #[tokio::test]
        async fn should_count_only_the_rent_above_the_reserve() {
            for (rent, count, lamports) in [
                (0, 0, 0),
                (RENT_RESERVE, 0, 0),
                (RENT_RESERVE + 160_720, 1, 160_720),
            ] {
                let signature = setup_sent_withdrawal();

                if rent > RENT_RESERVE {
                    finalize_in_two_rounds(outcome(&signature, Some(rent), true)).await;
                } else {
                    finalize(outcome(&signature, Some(rent), true)).await;
                }

                let mut expected = failed_withdrawals(0).to_vec();
                expected.push(format!("spl_withdrawals_above_rent_reserve {count} "));
                expected.push(format!("spl_rent_above_reserve {lamports} "));
                assert_metrics(0, &expected);
                assert_replay();
            }
        }

        #[tokio::test]
        async fn should_not_count_rent_above_the_reserve_for_a_failed_withdrawal() {
            let signature = setup_sent_withdrawal();

            finalize(outcome(&signature, None, true)).await;

            assert_metrics(
                0,
                &[
                    "spl_withdrawals_above_rent_reserve 0 ".to_string(),
                    "spl_rent_above_reserve 0 ".to_string(),
                ],
            );
        }

        #[tokio::test]
        async fn should_report_the_oldest_withdrawal_and_the_free_nonce_accounts() {
            let age =
                |seconds: u64| format!("oldest_incomplete_spl_withdrawal_age_seconds {seconds} ");
            let free = |accounts: u64| format!("free_nonce_accounts {accounts} ");

            // Withdrawal 42 is accepted at 1 ns and sent. It uses the one nonce account.
            let signature = setup_sent_withdrawal();
            assert_metrics(90 * SECOND + 1, &[age(90), free(0)]);

            // Withdrawal 43 is accepted later and waits for a nonce account.
            record_at(
                20 * SECOND,
                EventType::AcceptedSplWithdrawalRequest(SplWithdrawalRequest {
                    terms: SplWithdrawalRequestTerms {
                        amount: 100,
                        ..terms()
                    },
                    cksol_burn_block_index: 43.into(),
                    ckspl_burn_block_index: 8.into(),
                }),
            );
            assert_metrics(90 * SECOND + 1, &[age(90), free(0)]);

            // The settlement of 42 leaves the pending withdrawal 43 as the oldest.
            finalize(outcome(&signature, Some(RENT_RESERVE), true)).await;
            assert_metrics(50 * SECOND, &[age(30), free(1)]);

            // A withdrawal with a created transaction is still incomplete.
            record_at(
                40 * SECOND,
                EventType::CreatedSplWithdrawalTransaction {
                    cksol_burn_block_index: 43.into(),
                    nonce_account: NONCE_ACCOUNT,
                    nonce_value: durable_nonce(2),
                },
            );
            assert_metrics(50 * SECOND, &[age(30), free(0)]);
        }

        #[tokio::test]
        async fn should_report_no_age_when_every_withdrawal_is_settled() {
            let signature = setup_sent_withdrawal();

            finalize(outcome(&signature, None, true)).await;

            assert_metrics(
                90 * SECOND,
                &[
                    "oldest_incomplete_spl_withdrawal_age_seconds 0 ".to_string(),
                    "free_nonce_accounts 1 ".to_string(),
                ],
            );
        }

        #[tokio::test]
        async fn should_report_quarantined_rent_refunds_and_token_remints() {
            let mint = mint();
            let expected = |quarantined: u64| {
                [
                    format!(
                        r#"quarantined_spl_withdrawal_reimbursements{{mint="{mint}"}} {quarantined} "#
                    ),
                    format!(
                        r#"quarantined_spl_cksol_refunds{{kind="failed_transfer_rent"}} {quarantined} "#
                    ),
                    r#"quarantined_spl_cksol_refunds{kind="unused_rent"} 0 "#.to_string(),
                    r#"quarantined_spl_cksol_refunds{kind="failed_token_burn"} 0 "#.to_string(),
                ]
            };
            let signature = setup_sent_withdrawal();
            finalize(outcome(&signature, None, true)).await;
            // A pending remint and a pending refund are not quarantined.
            assert_metrics(0, &expected(0));

            process_pending_reimbursements(reimbursement_runtime([Err(TransferError::TooOld)]))
                .await;
            process_pending_cksol_refunds(reimbursement_runtime([Err(TransferError::TooOld)]))
                .await;

            assert_metrics(0, &expected(1));
        }

        #[tokio::test]
        async fn should_report_the_shortfall_of_the_operating_balance() {
            let signature = setup_sent_withdrawal();
            // The fee less the transaction fee absorbs the rent above the reserve.
            let rent = RENT_RESERVE + FEE - FEE_PER_SIGNATURE + 7;

            finalize_in_two_rounds(outcome(&signature, Some(rent), true)).await;

            let mut encoder = ic_metrics_encoder::MetricsEncoder::new(Vec::new(), 0);
            read_state(|state| {
                crate::metrics::encode_operating_balance_metrics(&mut encoder, state)
            })
            .unwrap();
            let metrics = String::from_utf8(encoder.into_inner()).unwrap();
            for line in ["operating_balance 0 ", "operating_balance_shortfall 7 "] {
                assert!(metrics.contains(line), "missing {line} in {metrics}");
            }
        }
    }
}
