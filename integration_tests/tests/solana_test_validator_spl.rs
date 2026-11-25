use assert_matches::assert_matches;
use assert2::check;
use cksol_int_tests::{
    Ledger, Setup,
    fixtures::{DEFAULT_CALLER_ACCOUNT, MINTER_ADDRESS},
    validator::{FEE_PER_SIGNATURE, SolanaTestValidator},
};
use cksol_types::{
    DepositSplArgs, DepositSplError, DepositSplId, DepositSplStatus, Signature,
    SplRentRefundStatus, TxFinalizedStatus, WithdrawSplArgs, WithdrawSplStatus,
};
use cksol_types_internal::{SplTokenRegistration, UpgradeArgs, event::EventType};
use icrc_ledger_types::icrc1::account::Account;
use solana_address::Address;
use solana_keypair::{Keypair, Signer};
use solana_native_token::LAMPORTS_PER_SOL;
use spl_associated_token_account_interface::address::get_associated_token_address;
use std::time::Duration;

const DECIMALS: u8 = 6;
const DEPOSIT_AMOUNT: u64 = 10_000_000;
const DEPOSIT_FEE: u64 = 1_800;

#[tokio::test(flavor = "multi_thread")]
async fn should_sweep_and_mint_a_classic_spl_deposit_on_the_validator() {
    let validator = SolanaTestValidator::start().await;
    let setup = validator.setup().await;
    let (ledger, funder, mint, treasury) = register_mint(&validator, &setup).await;

    let account = Account {
        owner: Setup::DEFAULT_CALLER,
        subaccount: Some([1; 32]),
    };
    let owner: Address = setup.minter().get_deposit_address(account).await.into();
    let source = validator
        .mint_to(&funder, &mint, &owner, DEPOSIT_AMOUNT)
        .await;
    assert_eq!(validator.get_token_balance(&source).await, DEPOSIT_AMOUNT);
    assert_eq!(validator.get_token_balance(&treasury).await, 0);
    let sol_before = validator.get_balance(&MINTER_ADDRESS).await;
    let tracked_sol_before = setup.minter().get_minter_info().await.balance;
    let operating_before = setup.minter().get_operating_balance().await.balance;

    let deposit_id = setup
        .minter()
        .deposit_spl(DepositSplArgs {
            owner: None,
            subaccount: account.subaccount,
            mint: mint.into(),
        })
        .await
        .expect("deposit_spl should read the real source balance and apply the fixed fee");
    assert_eq!(
        setup.minter().deposit_spl_status(deposit_id).await,
        DepositSplStatus::Queued {
            balance: DEPOSIT_AMOUNT,
            fee: DEPOSIT_FEE,
        }
    );
    assert_eq!(ledger.balance_of(account).await, 0);

    // The minter signs and submits the sweep; the validator executes the real SPL transfer.
    setup.advance_time(Duration::from_mins(1)).await;
    validator
        .wait_for_finalized_token_balance(&treasury, DEPOSIT_AMOUNT)
        .await;
    assert_eq!(validator.get_token_balance(&source).await, 0);
    let sweep_fee = 2 * FEE_PER_SIGNATURE;
    assert_eq!(
        validator.get_balance(&MINTER_ADDRESS).await,
        sol_before - sweep_fee
    );

    // The minter settles the validator's transaction metadata, then mints the net amount.
    assert_eq!(
        setup.wait_for_spl_deposit_minted(deposit_id).await,
        DEPOSIT_AMOUNT - DEPOSIT_FEE
    );
    assert_eq!(
        setup.minter().deposit_spl_status(deposit_id).await,
        DepositSplStatus::Minted {
            block_index: 0,
            minted_amount: DEPOSIT_AMOUNT - DEPOSIT_FEE,
        }
    );
    assert_eq!(
        ledger.balance_of(account).await,
        DEPOSIT_AMOUNT - DEPOSIT_FEE
    );
    assert_eq!(
        setup.minter().get_minter_info().await.balance,
        tracked_sol_before - sweep_fee
    );
    assert_eq!(
        setup.minter().get_operating_balance().await.balance,
        operating_before - sweep_fee
    );

    setup.drop().await;
}

/// The source is frozen after the request, so the preflight simulation rejects the sweep
/// and the transaction never lands.
#[tokio::test(flavor = "multi_thread")]
async fn should_drop_an_spl_deposit_when_its_sweep_expires_on_the_validator() {
    let validator = SolanaTestValidator::start().await;
    let setup = validator.setup().await;
    let (ledger, funder, mint, treasury) = register_mint(&validator, &setup).await;

    let account = Account {
        owner: Setup::DEFAULT_CALLER,
        subaccount: Some([1; 32]),
    };
    let args = DepositSplArgs {
        owner: None,
        subaccount: account.subaccount,
        mint: mint.into(),
    };
    let owner: Address = setup.minter().get_deposit_address(account).await.into();
    let source = validator
        .mint_to(&funder, &mint, &owner, DEPOSIT_AMOUNT)
        .await;
    let sol_before = validator.get_balance(&MINTER_ADDRESS).await;
    let tracked_sol_before = setup.minter().get_minter_info().await.balance;
    let operating_before = setup.minter().get_operating_balance().await.balance;

    let deposit_id = setup
        .minter()
        .deposit_spl(args.clone())
        .await
        .expect("deposit_spl should queue the deposit");
    validator
        .freeze_token_account(&funder, &mint, &source)
        .await;

    let signature = wait_for_spl_deposit_dropped(&setup, deposit_id).await;

    setup.minter().assert_that_events().await.satisfy(|events| {
        check!(events.contains(&EventType::ExpiredTransaction {
            signature: signature.clone()
        }));
        check!(!events.iter().any(|event| matches!(
            event,
            EventType::CreditedSplSweep { .. } | EventType::FailedSplSweep { .. }
        )));
    });
    assert_eq!(validator.get_token_balance(&source).await, DEPOSIT_AMOUNT);
    assert_eq!(validator.get_token_balance(&treasury).await, 0);
    assert_eq!(ledger.balance_of(account).await, 0);
    // The transaction never landed, so no fee was paid and the reservation returned.
    assert_eq!(validator.get_balance(&MINTER_ADDRESS).await, sol_before);
    assert_eq!(
        setup.minter().get_minter_info().await.balance,
        tracked_sol_before
    );
    assert_eq!(
        setup.minter().get_operating_balance().await.balance,
        operating_before
    );
    // The pair is released: the new request reads the source again.
    assert_matches!(
        setup.minter().deposit_spl(args).await,
        Err(DepositSplError::InvalidTokenAccount(reason)) if reason.contains("frozen")
    );

    // The minter classified the text of the real preflight rejection.
    setup
        .check_metrics()
        .await
        .assert_contains_metric_matching(format!(
            r#"failed_spl_sweeps\{{mint="{mint}",reason="frozen_account"\}} 1 \d+"#
        ))
        .assert_contains_metric_matching(format!(
            r#"dropped_spl_deposits\{{mint="{mint}"\}} 1 \d+"#
        ))
        .into()
        .drop()
        .await;
}

/// The real transaction metadata of the validator settles both rent cases: a destination
/// token account that the withdrawal creates, and one that exists.
#[tokio::test(flavor = "multi_thread")]
async fn should_withdraw_spl_to_a_new_and_then_an_existing_token_account_on_the_validator() {
    const FEE: u64 = 1_000_000;
    const RENT_RESERVE: u64 = 2_039_280;

    let validator = SolanaTestValidator::start().await;
    let setup = validator.setup().await;
    let (ledger, funder, mint, treasury) = register_mint(&validator, &setup).await;

    // The default caller holds ckSOL from the funding deposit. It deposits the tokens.
    let account = DEFAULT_CALLER_ACCOUNT;
    let owner: Address = setup.minter().get_deposit_address(account).await.into();
    validator
        .mint_to(&funder, &mint, &owner, DEPOSIT_AMOUNT)
        .await;
    let deposit_id = setup
        .minter()
        .deposit_spl(DepositSplArgs {
            owner: None,
            subaccount: None,
            mint: mint.into(),
        })
        .await
        .expect("deposit_spl should queue the deposit");
    setup.advance_time(Duration::from_mins(1)).await;
    validator
        .wait_for_finalized_token_balance(&treasury, DEPOSIT_AMOUNT)
        .await;
    assert_eq!(
        setup.wait_for_spl_deposit_minted(deposit_id).await,
        DEPOSIT_AMOUNT - DEPOSIT_FEE
    );
    setup
        .ledger()
        .approve(None, 100 * FEE, setup.minter_account())
        .await;
    ledger
        .approve(None, DEPOSIT_AMOUNT, setup.minter_account())
        .await;

    let destination = Keypair::new().pubkey();
    let destination_token_account =
        get_associated_token_address(&destination.to_bytes().into(), &mint.to_bytes().into())
            .to_bytes()
            .into();
    let finalized = |status: &WithdrawSplStatus| {
        matches!(
            status,
            WithdrawSplStatus::TxFinalized {
                rent_refund: SplRentRefundStatus::NotDue | SplRentRefundStatus::Reimbursed { .. },
                ..
            }
        )
    };

    // 1. The destination has no token account: the transaction creates it.
    let cksol_before = setup.ledger().balance_of(account).await;
    let ckspl_before = ledger.balance_of(account).await;
    let sol_before = validator.get_balance(&MINTER_ADDRESS).await;
    let operating_before = setup.minter().get_operating_balance().await.balance;
    let first = setup
        .minter()
        .withdraw_spl(WithdrawSplArgs {
            mint: mint.into(),
            from_subaccount: None,
            amount: 1_000_000,
            address: destination.to_string(),
        })
        .await
        .expect("withdraw_spl should accept the request");
    assert_eq!(
        setup.ledger().balance_of(account).await,
        cksol_before - FEE - RENT_RESERVE
    );
    assert_eq!(
        ledger.balance_of(account).await,
        ckspl_before - 1_000_000_u64
    );
    let status = setup
        .wait_for_spl_withdrawal(first.cksol_burn_block_index, finalized)
        .await;
    assert_matches!(
        status,
        WithdrawSplStatus::TxFinalized {
            transaction: TxFinalizedStatus::Success { .. },
            rent_refund: SplRentRefundStatus::NotDue,
        }
    );
    assert_eq!(
        validator
            .get_token_balance(&destination_token_account)
            .await,
        1_000_000
    );
    assert_eq!(
        validator.get_token_balance(&treasury).await,
        DEPOSIT_AMOUNT - 1_000_000
    );
    // The minter paid the transaction fee and the rent of the new account.
    assert_eq!(
        validator.get_balance(&MINTER_ADDRESS).await,
        sol_before - FEE_PER_SIGNATURE - RENT_RESERVE
    );
    assert_eq!(
        validator.get_balance(&destination_token_account).await,
        RENT_RESERVE
    );
    assert_eq!(
        setup.minter().get_operating_balance().await.balance,
        operating_before + FEE - FEE_PER_SIGNATURE
    );
    // The whole reserve was spent, so no ckSOL comes back.
    assert_eq!(
        setup.ledger().balance_of(account).await,
        cksol_before - FEE - RENT_RESERVE
    );

    // 2. The destination token account exists now: the rent reserve is refunded.
    let cksol_before = setup.ledger().balance_of(account).await;
    let sol_before = validator.get_balance(&MINTER_ADDRESS).await;
    let second = setup
        .minter()
        .withdraw_spl(WithdrawSplArgs {
            mint: mint.into(),
            from_subaccount: None,
            amount: 500_000,
            address: destination.to_string(),
        })
        .await
        .expect("withdraw_spl should accept the second request");
    assert_eq!(
        ledger.balance_of(account).await,
        ckspl_before - 1_500_000_u64
    );
    let status = setup
        .wait_for_spl_withdrawal(second.cksol_burn_block_index, finalized)
        .await;
    assert_matches!(
        status,
        WithdrawSplStatus::TxFinalized {
            transaction: TxFinalizedStatus::Success { .. },
            rent_refund: SplRentRefundStatus::Reimbursed {
                amount: RENT_RESERVE,
                ..
            },
        }
    );
    assert_eq!(
        validator
            .get_token_balance(&destination_token_account)
            .await,
        1_500_000
    );
    assert_eq!(
        validator.get_balance(&MINTER_ADDRESS).await,
        sol_before - FEE_PER_SIGNATURE
    );
    // The settlement mints no ckSPL back.
    assert_eq!(
        ledger.balance_of(account).await,
        ckspl_before - 1_500_000_u64
    );
    // Only the fee is kept: the rent reserve is minted back.
    assert_eq!(setup.ledger().balance_of(account).await, cksol_before - FEE);

    setup.drop().await;
}

/// Waits for the validator to pass the expiry window of the sweep, in real time.
async fn wait_for_spl_deposit_dropped(setup: &Setup, deposit_id: DepositSplId) -> Signature {
    const MAX_ITERATIONS: usize = 120;
    for _ in 0..MAX_ITERATIONS {
        if let DepositSplStatus::Dropped { signature } =
            setup.minter().deposit_spl_status(deposit_id).await
        {
            return signature;
        }
        setup.advance_time_and_settle(Duration::from_mins(1)).await;
    }
    panic!("SPL deposit {deposit_id} was not dropped within timeout");
}

/// Funds the minter and its operating balance, creates a classic SPL mint with its
/// treasury account on the validator and registers the mint on a new token ledger.
async fn register_mint<'a>(
    validator: &SolanaTestValidator,
    setup: &'a Setup,
) -> (Ledger<'a>, Keypair, Address, Address) {
    validator.fund_minter(setup).await;
    setup.top_up_operating_balance().await;
    let ledger = setup.install_spl_ledger(DECIMALS).await;

    let funder = Keypair::new();
    validator
        .airdrop_and_confirm(funder.pubkey(), LAMPORTS_PER_SOL)
        .await;
    let mint = validator.create_mint(&funder, DECIMALS).await;
    // The minter does not create its treasury account; an operator does before registration.
    let treasury = validator
        .create_token_account(&funder, &mint, &MINTER_ADDRESS)
        .await;
    setup
        .minter()
        .upgrade(UpgradeArgs {
            add_spl_tokens: Some(vec![SplTokenRegistration {
                mint: mint.to_string(),
                token_program: spl_token_interface::id().to_string(),
                decimals: DECIMALS,
                ledger_id: ledger.canister_id(),
                index_id: None,
                symbol: "ckSPL".to_string(),
                minimum_deposit_amount: 1_000_000,
                deposit_fee: DEPOSIT_FEE,
                deposits_paused: false,
                withdrawal_fee_token: None,
            }]),
            ..UpgradeArgs::default()
        })
        .await
        .expect("upgrade should register the token");
    (ledger, funder, mint, treasury)
}
