use assert_matches::assert_matches;
use assert2::check;
use cksol_int_tests::{
    Ledger, Setup, SetupBuilder,
    fixtures::{
        FINALIZE_TRANSACTIONS_DELAY, MINTER_ADDRESS, MockBuilder, WITHDRAWAL_PROCESSING_DELAY,
        get_memo,
    },
    ledger_init_args::LEDGER_TRANSFER_FEE,
    validator::FEE_PER_SIGNATURE,
};
use cksol_types::{
    CollectedSplWithdrawalFees, Lamport, Memo, Signature, SplRentRefundStatus, TxFinalizedStatus,
    WithdrawSolArgs, WithdrawSolStatus, WithdrawSplArgs, WithdrawSplError, WithdrawSplLedgerError,
    WithdrawSplOk, WithdrawSplStatus,
};
use cksol_types_internal::{
    UpgradeArgs,
    event::{EventType, TransactionPurpose},
};
use icrc_ledger_types::icrc1::account::Account;
use solana_address::{Address, address};

mod fixtures;

use fixtures::spl::{
    BALANCE_ABOVE_MINIMUM, DECIMALS, DEPOSIT_FEE, MINT, OTHER_MINT, RENT_EXEMPT_LAMPORTS,
    WithdrawalOutcome, finalize_sweep, queue_deposit, register_token, run_mint_timer,
    setup_with_registered_mint, spl_token_registration, sweep_queued_deposit,
    withdrawal_transaction_response,
};

const DESTINATION: Address = address!("E4MpwNnMWs2XtW5gVrxZvyS7fMq31QD5HvbxmwP45Tz3");
const AMOUNT: u64 = 400_000;
const FEE: Lamport = Setup::DEFAULT_WITHDRAWAL_FEE;
const RENT_RESERVE: Lamport = RENT_EXEMPT_LAMPORTS;
/// The ckSPL that the account holds after its deposit is minted and the approval is paid.
const TOKENS: u64 = BALANCE_ABOVE_MINIMUM - DEPOSIT_FEE - LEDGER_TRANSFER_FEE;
/// The ckSOL ledger fee that the minter deducts from a fee refund. It is the fee of the
/// production ledger; the test ledger charges less.
const CKSOL_LEDGER_FEE: Lamport = 500;

fn withdrawal_args(amount: u64) -> WithdrawSplArgs {
    WithdrawSplArgs {
        mint: MINT.into(),
        from_subaccount: None,
        amount,
        address: DESTINATION.to_string(),
    }
}

/// Funds the minter, registers [`MINT`] and mints the test deposit to the caller, who
/// then holds ckSOL and ckSPL. The calls use RPC IDs 0 to 63.
async fn setup_with_tokens() -> (Setup, Account) {
    let setup = SetupBuilder::new().with_proxy_canister().build().await;
    setup.fund_minter().await;
    setup.top_up_operating_balance().await;
    let account = Account {
        owner: setup.proxy_canister_id(),
        subaccount: None,
    };
    (setup, account)
}

/// Mints the test deposit of the account on the token ledger and approves the minter on
/// both ledgers. The calls use RPC IDs 32 to 63.
async fn deposit_tokens_and_approve(setup: &Setup, token_ledger: &Ledger<'_>, account: Account) {
    register_token(setup, spl_token_registration(token_ledger.canister_id())).await;
    let owner: Address = setup.minter().get_deposit_address(account).await.into();
    let deposit_id = queue_deposit(setup, account, 32).await;
    let signature = sweep_queued_deposit(setup, deposit_id, 36).await;
    finalize_sweep(setup, &signature, &owner, 48).await;
    run_mint_timer(setup).await;
    setup
        .ledger()
        .approve(None, 10 * (FEE + RENT_RESERVE), setup.minter_account())
        .await;
    token_ledger
        .approve(None, TOKENS, setup.minter_account())
        .await;
    assert_eq!(token_ledger.balance_of(account).await, TOKENS);
}

/// Runs the withdrawal timer, which reads the nonce account and submits the transaction
/// with RPC IDs 64 to 71. Returns the signature of the transaction.
async fn submit_withdrawal(setup: &Setup, cksol_burn_block_index: u64) -> Signature {
    setup.advance_time(WITHDRAWAL_PROCESSING_DELAY).await;
    setup
        .execute_http_mocks(
            MockBuilder::with_start_id(64)
                .submit_withdrawal_transaction()
                .build(),
        )
        .await;
    assert_matches!(
        setup.minter().withdraw_spl_status(cksol_burn_block_index).await,
        WithdrawSplStatus::TxSent { transaction_id } => transaction_id
    )
}

/// Runs the finalization timer, which reads the advanced nonce account and the landed
/// transaction with RPC IDs 72 to 79.
async fn finalize_withdrawal(setup: &Setup, signature: &Signature, outcome: WithdrawalOutcome) {
    finalize_withdrawal_from(setup, signature, outcome, 72).await;
}

/// As [`finalize_withdrawal`], with RPC IDs `start_id` to `start_id + 7`.
async fn finalize_withdrawal_from(
    setup: &Setup,
    signature: &Signature,
    outcome: WithdrawalOutcome,
    start_id: u64,
) {
    let withdrawal = setup.minter().signed_transaction(signature).await;
    setup.advance_time(FINALIZE_TRANSACTIONS_DELAY).await;
    setup
        .execute_http_mocks(
            MockBuilder::with_start_id(start_id)
                .get_advanced_nonce_account()
                .get_transaction(
                    signature,
                    withdrawal_transaction_response(&withdrawal, &DESTINATION, AMOUNT, outcome),
                )
                .build(),
        )
        .await;
}

/// Burns the fee and the tokens, sends the transfer on the timers, keeps it across an
/// upgrade and settles it.
#[tokio::test]
async fn should_burn_send_and_settle_a_withdrawal_after_timers() {
    let (setup, account) = setup_with_tokens().await;
    let token_ledger = setup.install_spl_ledger(DECIMALS).await;
    deposit_tokens_and_approve(&setup, &token_ledger, account).await;
    let cksol_before = setup.ledger().balance_of(account).await;
    let info = setup.minter().get_minter_info().await;
    let balance_before = info.balance;
    let operating_before = setup.minter().get_operating_balance().await.balance;
    // The token info tells a client how much ckSOL a withdrawal burns.
    let token = &info.supported_spl_tokens.as_ref().unwrap()[0];
    assert_eq!(
        (
            token.withdrawal_fee_cksol,
            token.refundable_rent,
            token.total_withdrawal_fee_cksol
        ),
        (FEE, RENT_RESERVE, FEE + RENT_RESERVE)
    );
    assert_eq!(
        info.collected_spl_withdrawal_fees,
        Some(vec![CollectedSplWithdrawalFees {
            mint: MINT.into(),
            amount: 0,
        }])
    );

    let WithdrawSplOk {
        cksol_burn_block_index,
        ckspl_burn_block_index,
    } = setup
        .minter()
        .withdraw_spl(withdrawal_args(AMOUNT))
        .await
        .expect("withdraw_spl should accept the request");

    assert_eq!(
        setup.ledger().balance_of(account).await,
        cksol_before - FEE - RENT_RESERVE
    );
    assert_eq!(token_ledger.balance_of(account).await, TOKENS - AMOUNT);
    assert_eq!(
        setup
            .minter()
            .withdraw_spl_status(cksol_burn_block_index)
            .await,
        WithdrawSplStatus::Pending
    );
    assert_eq!(
        setup
            .minter()
            .withdraw_spl_status(cksol_burn_block_index + 1)
            .await,
        WithdrawSplStatus::NotFound
    );
    // Each burn names the other part of the withdrawal in its memo.
    let fee_memo: Memo = minicbor::decode(&get_memo(
        setup.ledger().get_block(cksol_burn_block_index).await,
    ))
    .unwrap();
    assert_eq!(
        fee_memo,
        Memo::spl_withdrawal_fee(MINT, AMOUNT, DESTINATION)
    );
    let token_memo: Memo = minicbor::decode(&get_memo(
        token_ledger.get_block(ckspl_burn_block_index).await,
    ))
    .unwrap();
    assert_eq!(
        token_memo,
        Memo::spl_convert(cksol_burn_block_index, DESTINATION)
    );

    let signature = submit_withdrawal(&setup, cksol_burn_block_index).await;
    setup.minter().assert_that_events().await.satisfy(|events| {
        check!(events.iter().any(|event| matches!(
            event,
            EventType::CreatedSplWithdrawalTransaction {
                cksol_burn_block_index: index,
                nonce_account,
                ..
            } if *index == cksol_burn_block_index
                && nonce_account.to_string() == Setup::DEFAULT_NONCE_ACCOUNT.to_string()
        )));
        check!(events.iter().any(|event| matches!(
            event,
            EventType::SubmittedTransaction {
                purpose: TransactionPurpose::SplWithdrawal {
                    cksol_burn_block_index: index
                },
                ..
            } if *index == cksol_burn_block_index
        )));
    });

    // An upgrade replays the events of the in-flight withdrawal.
    setup
        .minter()
        .upgrade(UpgradeArgs::default())
        .await
        .expect("upgrade should succeed");
    assert_eq!(
        setup
            .minter()
            .withdraw_spl_status(cksol_burn_block_index)
            .await,
        WithdrawSplStatus::TxSent {
            transaction_id: signature.clone()
        }
    );

    finalize_withdrawal(&setup, &signature, WithdrawalOutcome::CreatedAccount).await;

    assert_eq!(
        setup
            .minter()
            .withdraw_spl_status(cksol_burn_block_index)
            .await,
        WithdrawSplStatus::TxFinalized {
            transaction: TxFinalizedStatus::Success {
                transaction_id: signature.clone(),
            },
            rent_refund: SplRentRefundStatus::NotDue,
        }
    );
    setup.minter().assert_that_events().await.satisfy(|events| {
        check!(
            events.contains(&EventType::SucceededSplWithdrawalTransaction {
                signature: signature.clone(),
                lamports_spent: FEE_PER_SIGNATURE + RENT_RESERVE,
                rent_spent: RENT_RESERVE,
            })
        );
        check!(!events.iter().any(|event| matches!(
            event,
            EventType::QueuedSplCkSolRefund { .. }
                | EventType::FailedSplWithdrawalTransaction { .. }
        )));
    });
    // The main balance paid the transaction fee and the rent. The fee less the
    // transaction fee went to the operating balance.
    assert_eq!(
        setup.minter().get_minter_info().await.balance,
        balance_before - FEE_PER_SIGNATURE - RENT_RESERVE
    );
    assert_eq!(
        setup.minter().get_operating_balance().await.balance,
        operating_before + FEE - FEE_PER_SIGNATURE
    );
    // The whole rent reserve was spent, so the timers mint nothing back.
    run_mint_timer(&setup).await;
    assert_eq!(
        setup.ledger().balance_of(account).await,
        cksol_before - FEE - RENT_RESERVE
    );
    assert_eq!(token_ledger.balance_of(account).await, TOKENS - AMOUNT);

    setup.drop().await;
}

/// A transfer that fails on-chain mints the burned tokens and the rent reserve back, and
/// keeps the fee.
#[tokio::test]
async fn should_remint_the_tokens_and_refund_the_rent_when_the_transfer_fails_on_chain() {
    let (setup, account) = setup_with_tokens().await;
    let token_ledger = setup.install_spl_ledger(DECIMALS).await;
    deposit_tokens_and_approve(&setup, &token_ledger, account).await;
    let cksol_before = setup.ledger().balance_of(account).await;
    let balance_before = setup.minter().get_minter_info().await.balance;
    let operating_before = setup.minter().get_operating_balance().await.balance;
    let WithdrawSplOk {
        cksol_burn_block_index,
        ..
    } = setup
        .minter()
        .withdraw_spl(withdrawal_args(AMOUNT))
        .await
        .expect("withdraw_spl should accept the request");
    let signature = submit_withdrawal(&setup, cksol_burn_block_index).await;

    finalize_withdrawal(&setup, &signature, WithdrawalOutcome::Failed).await;

    setup.minter().assert_that_events().await.satisfy(|events| {
        check!(events.contains(&EventType::FailedSplWithdrawalTransaction {
            signature: signature.clone(),
            lamports_spent: FEE_PER_SIGNATURE,
        }));
    });
    let status = setup
        .wait_for_spl_withdrawal(cksol_burn_block_index, |status| {
            matches!(
                status,
                WithdrawSplStatus::Reimbursed {
                    rent_refund: SplRentRefundStatus::Reimbursed { .. },
                    ..
                }
            )
        })
        .await;
    assert_matches!(
        status,
        WithdrawSplStatus::Reimbursed {
            transaction_id,
            reimbursed_amount: AMOUNT,
            rent_refund: SplRentRefundStatus::Reimbursed {
                amount: RENT_RESERVE,
                ..
            },
            ..
        } if transaction_id == signature
    );
    // The user has all the tokens again and paid only the fee.
    assert_eq!(token_ledger.balance_of(account).await, TOKENS);
    assert_eq!(setup.ledger().balance_of(account).await, cksol_before - FEE);
    // The main balance paid only the transaction fee, which the kept fee covers.
    assert_eq!(
        setup.minter().get_minter_info().await.balance,
        balance_before - FEE_PER_SIGNATURE
    );
    assert_eq!(
        setup.minter().get_operating_balance().await.balance,
        operating_before + FEE - FEE_PER_SIGNATURE
    );

    setup
        .check_metrics()
        .await
        .assert_contains_metric_matching(format!(
            r#"failed_spl_withdrawals\{{mint="{MINT}",reason="frozen_account"\}} 1 \d+"#
        ))
        .assert_contains_metric_matching(format!(
            r#"failed_spl_withdrawals\{{mint="{MINT}",reason="other"\}} 0 \d+"#
        ))
        .assert_contains_metric_matching(r"oldest_incomplete_spl_withdrawal_age_seconds 0 \d+")
        .assert_contains_metric_matching(r"free_nonce_accounts 1 \d+")
        .assert_contains_metric_matching(r"operating_balance_shortfall 0 \d+")
        .into()
        .drop()
        .await;
}

/// A rent above the reserve halts SPL withdrawals. Only an upgrade with
/// `spl_withdrawals_halted = false` resumes them.
#[tokio::test]
async fn should_halt_withdrawals_after_a_rent_above_the_reserve_until_an_upgrade_resumes() {
    let (setup, account) = setup_with_tokens().await;
    let token_ledger = setup.install_spl_ledger(DECIMALS).await;
    deposit_tokens_and_approve(&setup, &token_ledger, account).await;
    assert_eq!(
        setup
            .minter()
            .get_minter_info()
            .await
            .spl_withdrawals_halted,
        Some(false)
    );
    let WithdrawSplOk {
        cksol_burn_block_index,
        ..
    } = setup
        .minter()
        .withdraw_spl(withdrawal_args(AMOUNT))
        .await
        .expect("withdraw_spl should accept the request");
    let signature = submit_withdrawal(&setup, cksol_burn_block_index).await;

    finalize_withdrawal(
        &setup,
        &signature,
        WithdrawalOutcome::CreatedAccountAboveReserve,
    )
    .await;

    // Round 1: the minter halts. The withdrawal is not settled yet.
    assert_eq!(
        setup
            .minter()
            .get_minter_info()
            .await
            .spl_withdrawals_halted,
        Some(true)
    );
    assert_eq!(
        setup
            .minter()
            .withdraw_spl_status(cksol_burn_block_index)
            .await,
        WithdrawSplStatus::TxSent {
            transaction_id: signature.clone()
        }
    );

    // Round 2: the withdrawal is settled.
    finalize_withdrawal_from(
        &setup,
        &signature,
        WithdrawalOutcome::CreatedAccountAboveReserve,
        80,
    )
    .await;
    setup.minter().assert_that_events().await.satisfy(|events| {
        check!(
            events.contains(&EventType::SucceededSplWithdrawalTransaction {
                signature: signature.clone(),
                lamports_spent: FEE_PER_SIGNATURE + RENT_RESERVE + 1,
                rent_spent: RENT_RESERVE + 1,
            })
        );
        check!(events.contains(&EventType::HaltedSplWithdrawals {
            signature: signature.clone(),
            rent_spent: RENT_RESERVE + 1,
            rent_reserve: RENT_RESERVE,
        }));
    });
    assert_matches!(
        setup
            .minter()
            .withdraw_spl_status(cksol_burn_block_index)
            .await,
        WithdrawSplStatus::TxFinalized {
            rent_refund: SplRentRefundStatus::NotDue,
            ..
        }
    );
    assert_eq!(
        setup
            .minter()
            .get_minter_info()
            .await
            .spl_withdrawals_halted,
        Some(true)
    );

    // A new request is rejected, and nothing is burned.
    let cksol_before = setup.ledger().balance_of(account).await;
    let tokens_before = token_ledger.balance_of(account).await;
    assert_eq!(
        setup.minter().withdraw_spl(withdrawal_args(1_000)).await,
        Err(WithdrawSplError::WithdrawalsHalted)
    );
    assert_eq!(setup.ledger().balance_of(account).await, cksol_before);
    assert_eq!(token_ledger.balance_of(account).await, tokens_before);

    // A normal upgrade does not resume.
    setup
        .minter()
        .upgrade(UpgradeArgs::default())
        .await
        .expect("upgrade should succeed");
    assert_eq!(
        setup
            .minter()
            .get_minter_info()
            .await
            .spl_withdrawals_halted,
        Some(true)
    );
    assert_eq!(
        setup.minter().withdraw_spl(withdrawal_args(1_000)).await,
        Err(WithdrawSplError::WithdrawalsHalted)
    );
    let setup = setup
        .check_metrics()
        .await
        .assert_contains_metric_matching(r"spl_withdrawals_halted 1 \d+")
        .assert_contains_metric_matching(r"spl_withdrawals_above_rent_reserve 1 \d+")
        .assert_contains_metric_matching(r"spl_rent_above_reserve 1 \d+")
        .into();

    setup
        .minter()
        .upgrade(UpgradeArgs {
            spl_withdrawals_halted: Some(false),
            ..Default::default()
        })
        .await
        .expect("upgrade should succeed");

    assert_eq!(
        setup
            .minter()
            .get_minter_info()
            .await
            .spl_withdrawals_halted,
        Some(false)
    );
    let WithdrawSplOk {
        cksol_burn_block_index,
        ..
    } = setup
        .minter()
        .withdraw_spl(withdrawal_args(1_000))
        .await
        .expect("withdraw_spl should accept the request after the resume");
    assert_eq!(
        setup
            .minter()
            .withdraw_spl_status(cksol_burn_block_index)
            .await,
        WithdrawSplStatus::Pending
    );

    setup.drop().await;
}

/// A rent far above the reserve makes the settlement trap. The minter must stay halted.
#[tokio::test]
async fn should_keep_the_halt_when_the_settlement_traps_on_a_low_balance() {
    let (setup, account) = setup_with_tokens().await;
    let token_ledger = setup.install_spl_ledger(DECIMALS).await;
    deposit_tokens_and_approve(&setup, &token_ledger, account).await;
    let WithdrawSplOk {
        cksol_burn_block_index,
        ..
    } = setup
        .minter()
        .withdraw_spl(withdrawal_args(AMOUNT))
        .await
        .expect("withdraw_spl should accept the request");
    let signature = submit_withdrawal(&setup, cksol_burn_block_index).await;
    let unreserved_balance = setup.minter().get_minter_info().await.balance;
    let rent = RENT_RESERVE + unreserved_balance + 1;
    let in_flight = WithdrawSplStatus::TxSent {
        transaction_id: signature.clone(),
    };

    // Round 1: the minter halts.
    finalize_withdrawal(
        &setup,
        &signature,
        WithdrawalOutcome::CreatedAccountWithRent(rent),
    )
    .await;
    assert_eq!(
        setup
            .minter()
            .get_minter_info()
            .await
            .spl_withdrawals_halted,
        Some(true)
    );
    let events_after_the_halt = setup.minter().get_all_events().await.len();

    // Round 2: the settlement traps.
    finalize_withdrawal_from(
        &setup,
        &signature,
        WithdrawalOutcome::CreatedAccountWithRent(rent),
        80,
    )
    .await;

    // The trap undid round 2 only. The withdrawal is not settled, and the minter is
    // still halted.
    assert_eq!(
        setup
            .minter()
            .withdraw_spl_status(cksol_burn_block_index)
            .await,
        in_flight
    );
    assert_eq!(
        setup.minter().get_minter_info().await.balance,
        unreserved_balance
    );
    assert_eq!(
        setup.minter().get_all_events().await.len(),
        events_after_the_halt
    );
    setup.minter().assert_that_events().await.satisfy(|events| {
        check!(events.contains(&EventType::HaltedSplWithdrawals {
            signature: signature.clone(),
            rent_spent: rent,
            rent_reserve: RENT_RESERVE,
        }));
        check!(
            !events
                .iter()
                .any(|event| matches!(event, EventType::SucceededSplWithdrawalTransaction { .. }))
        );
    });
    assert_eq!(
        setup
            .minter()
            .get_minter_info()
            .await
            .spl_withdrawals_halted,
        Some(true)
    );
    assert_eq!(
        setup.minter().withdraw_spl(withdrawal_args(1_000)).await,
        Err(WithdrawSplError::WithdrawalsHalted)
    );

    // The halt also survives an upgrade.
    setup
        .minter()
        .upgrade(UpgradeArgs::default())
        .await
        .expect("upgrade should succeed");
    assert_eq!(
        setup
            .minter()
            .withdraw_spl_status(cksol_burn_block_index)
            .await,
        in_flight
    );
    setup
        .check_metrics()
        .await
        .assert_contains_metric_matching(r"spl_withdrawals_halted 1 \d+")
        .assert_contains_metric_matching(r"spl_withdrawals_above_rent_reserve 0 \d+")
        .into()
        .drop()
        .await;
}

/// While halted, the minter keeps a queued SPL withdrawal and still sends a SOL
/// withdrawal. After the resume it sends the SPL withdrawal.
#[tokio::test]
async fn should_hold_a_queued_withdrawal_during_a_halt_and_send_it_after_the_resume() {
    const HELD_AMOUNT: u64 = 1_000;
    let (setup, account) = setup_with_tokens().await;
    let token_ledger = setup.install_spl_ledger(DECIMALS).await;
    deposit_tokens_and_approve(&setup, &token_ledger, account).await;
    let first = setup
        .minter()
        .withdraw_spl(withdrawal_args(AMOUNT))
        .await
        .expect("withdraw_spl should accept the first request")
        .cksol_burn_block_index;
    let first_signature = submit_withdrawal(&setup, first).await;
    // The first withdrawal uses the only nonce account, so the next two requests wait.
    let held = setup
        .minter()
        .withdraw_spl(withdrawal_args(HELD_AMOUNT))
        .await
        .expect("withdraw_spl should accept the second request")
        .cksol_burn_block_index;
    let sol = setup
        .minter()
        .withdraw_sol(WithdrawSolArgs {
            from_subaccount: None,
            amount: Setup::DEFAULT_MINIMUM_WITHDRAWAL_AMOUNT,
            address: DESTINATION.to_string(),
        })
        .await
        .expect("withdraw_sol should accept the request")
        .block_index;

    finalize_withdrawal(
        &setup,
        &first_signature,
        WithdrawalOutcome::CreatedAccountAboveReserve,
    )
    .await;
    // Round 1 halted the minter. Round 2 settles the first withdrawal.
    finalize_withdrawal_from(
        &setup,
        &first_signature,
        WithdrawalOutcome::CreatedAccountAboveReserve,
        80,
    )
    .await;
    assert_eq!(
        setup
            .minter()
            .get_minter_info()
            .await
            .spl_withdrawals_halted,
        Some(true)
    );

    // The SOL withdrawal is sent. The older SPL withdrawal is not.
    setup.advance_time(WITHDRAWAL_PROCESSING_DELAY).await;
    setup
        .execute_http_mocks(
            MockBuilder::with_start_id(88)
                .submit_withdrawal_transaction_after(1)
                .build(),
        )
        .await;
    let sol_signature = assert_matches!(
        setup.minter().withdraw_sol_status(sol).await,
        WithdrawSolStatus::TxSent { transaction_id } => transaction_id
    );
    assert_eq!(
        setup.minter().withdraw_spl_status(held).await,
        WithdrawSplStatus::Pending
    );
    let sol_withdrawal = setup.minter().signed_transaction(&sol_signature).await;
    setup.advance_time(FINALIZE_TRANSACTIONS_DELAY).await;
    setup
        .execute_http_mocks(
            MockBuilder::with_start_id(96)
                .finalize_withdrawal_transaction_after(&sol_withdrawal, 1)
                .build(),
        )
        .await;
    assert_matches!(
        setup.minter().withdraw_sol_status(sol).await,
        WithdrawSolStatus::TxFinalized(TxFinalizedStatus::Success { .. })
    );
    // The nonce account is free, but the SPL withdrawal still waits.
    setup.advance_time(WITHDRAWAL_PROCESSING_DELAY).await;
    assert_eq!(
        setup.minter().withdraw_spl_status(held).await,
        WithdrawSplStatus::Pending
    );

    setup
        .minter()
        .upgrade(UpgradeArgs {
            spl_withdrawals_halted: Some(false),
            ..Default::default()
        })
        .await
        .expect("upgrade should succeed");

    setup.advance_time(WITHDRAWAL_PROCESSING_DELAY).await;
    setup
        .execute_http_mocks(
            MockBuilder::with_start_id(104)
                .submit_withdrawal_transaction_after(2)
                .build(),
        )
        .await;
    let held_signature = assert_matches!(
        setup.minter().withdraw_spl_status(held).await,
        WithdrawSplStatus::TxSent { transaction_id } => transaction_id
    );
    let held_withdrawal = setup.minter().signed_transaction(&held_signature).await;
    setup.advance_time(FINALIZE_TRANSACTIONS_DELAY).await;
    setup
        .execute_http_mocks(
            MockBuilder::with_start_id(112)
                .get_nonce_account_after(3)
                .get_transaction(
                    &held_signature,
                    withdrawal_transaction_response(
                        &held_withdrawal,
                        &DESTINATION,
                        HELD_AMOUNT,
                        WithdrawalOutcome::CreatedAccount,
                    ),
                )
                .build(),
        )
        .await;

    assert_matches!(
        setup.minter().withdraw_spl_status(held).await,
        WithdrawSplStatus::TxFinalized {
            transaction: TxFinalizedStatus::Success { transaction_id },
            rent_refund: SplRentRefundStatus::NotDue,
        } if transaction_id == held_signature
    );
    // This rent is within the reserve, so the minter does not halt again.
    assert_eq!(
        setup
            .minter()
            .get_minter_info()
            .await
            .spl_withdrawals_halted,
        Some(false)
    );
    assert_eq!(
        token_ledger.balance_of(account).await,
        TOKENS - AMOUNT - HELD_AMOUNT
    );

    setup.drop().await;
}

#[tokio::test]
async fn should_reject_invalid_withdrawal_requests() {
    let setup = setup_with_registered_mint().await;
    let account = Account {
        owner: setup.proxy_canister_id(),
        subaccount: None,
    };
    let cksol_before = setup.ledger().balance_of(account).await;
    let events_before = setup.minter().get_all_events().await.len();

    assert_eq!(
        setup
            .minter()
            .withdraw_spl(WithdrawSplArgs {
                mint: OTHER_MINT.into(),
                ..withdrawal_args(AMOUNT)
            })
            .await,
        Err(WithdrawSplError::TokenNotSupported {
            mint: OTHER_MINT.into()
        })
    );
    assert_matches!(
        setup
            .minter()
            .withdraw_spl(WithdrawSplArgs {
                address: "not an address".to_string(),
                ..withdrawal_args(AMOUNT)
            })
            .await,
        Err(WithdrawSplError::MalformedAddress(_))
    );
    assert_matches!(
        setup
            .minter()
            .withdraw_spl(WithdrawSplArgs {
                address: MINTER_ADDRESS.to_string(),
                ..withdrawal_args(AMOUNT)
            })
            .await,
        Err(WithdrawSplError::InvalidDestination(_))
    );
    assert_matches!(
        setup.minter().withdraw_spl(withdrawal_args(0)).await,
        Err(WithdrawSplError::ValueTooSmall {
            withdrawal_amount: 0,
            ..
        })
    );
    // The minter has no allowance on the ckSOL ledger, so the fee burn fails.
    assert_matches!(
        setup.minter().withdraw_spl(withdrawal_args(AMOUNT)).await,
        Err(WithdrawSplError::CkSolLedgerError {
            error: WithdrawSplLedgerError::InsufficientAllowance { allowance, failed_burn_amount, .. }
        }) if allowance == 0_u8 && failed_burn_amount == FEE + RENT_RESERVE
    );

    // No rejected request burned ckSOL or recorded an event.
    assert_eq!(setup.ledger().balance_of(account).await, cksol_before);
    assert_eq!(setup.minter().get_all_events().await.len(), events_before);

    setup.drop().await;
}

/// The token burn fails after the fee burn, so the minter mints the fee and the rent
/// reserve back, less the ledger fee of the failed burn.
#[tokio::test]
async fn should_refund_the_fee_burn_when_the_token_burn_fails() {
    let setup = setup_with_registered_mint().await;
    let account = Account {
        owner: setup.proxy_canister_id(),
        subaccount: None,
    };
    setup
        .ledger()
        .approve(None, FEE + RENT_RESERVE, setup.minter_account())
        .await;
    let cksol_before = setup.ledger().balance_of(account).await;

    // The account holds no ckSPL and gave no allowance on the token ledger.
    let cksol_burn_block_index = assert_matches!(
        setup.minter().withdraw_spl(withdrawal_args(AMOUNT)).await,
        Err(WithdrawSplError::CkSplLedgerError {
            cksol_burn_block_index,
            error: WithdrawSplLedgerError::InsufficientAllowance { .. },
        }) => cksol_burn_block_index
    );

    let refund = FEE + RENT_RESERVE - CKSOL_LEDGER_FEE;
    assert_eq!(
        setup.ledger().balance_of(account).await,
        cksol_before - FEE - RENT_RESERVE
    );
    assert_eq!(
        setup
            .minter()
            .withdraw_spl_status(cksol_burn_block_index)
            .await,
        WithdrawSplStatus::FeeReimbursementPending {
            reimbursed_amount: refund
        }
    );

    let status = setup
        .wait_for_spl_withdrawal(cksol_burn_block_index, |status| {
            matches!(status, WithdrawSplStatus::FeeReimbursed { .. })
        })
        .await;
    assert_matches!(
        status,
        WithdrawSplStatus::FeeReimbursed {
            reimbursed_amount,
            ..
        } if reimbursed_amount == refund
    );
    assert_eq!(
        setup.ledger().balance_of(account).await,
        cksol_before - CKSOL_LEDGER_FEE
    );
    // The withdrawal never reaches the withdrawal queue.
    setup.minter().assert_that_events().await.satisfy(|events| {
        check!(
            !events
                .iter()
                .any(|event| matches!(event, EventType::AcceptedSplWithdrawalRequest { .. }))
        );
    });

    setup.drop().await;
}
