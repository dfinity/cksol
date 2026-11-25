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
    WithdrawSplArgs, WithdrawSplError, WithdrawSplLedgerError, WithdrawSplOk, WithdrawSplStatus,
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
    let withdrawal = setup.minter().signed_transaction(signature).await;
    setup.advance_time(FINALIZE_TRANSACTIONS_DELAY).await;
    setup
        .execute_http_mocks(
            MockBuilder::with_start_id(72)
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
