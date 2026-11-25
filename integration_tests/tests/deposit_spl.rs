use assert_matches::assert_matches;
use assert2::check;
use candid::Principal;
use cksol_int_tests::{
    Setup, SetupBuilder,
    fixtures::{
        DEFAULT_CALLER_ACCOUNT, DEFAULT_CALLER_DEPOSIT_ADDRESS, EXPIRY_BLOCK_HEIGHT,
        FINALIZE_TRANSACTIONS_DELAY, MockBuilder, SUBMISSION_BLOCK_HEIGHT, SWEEP_DEPOSITS_DELAY,
        get_memo,
    },
};
use cksol_types::{
    CollectedSplDepositFees, DepositSplArgs, DepositSplError, DepositSplStatus, Memo, MintMemo,
};
use cksol_types_internal::{SplTokenRegistration, UpgradeArgs, event::EventType};
use icrc_ledger_types::icrc1::account::Account;
use sol_rpc_types::{
    CommitmentLevel, ConsensusStrategy, GetAccountInfoEncoding, GetAccountInfoParams, RpcConfig,
};
use solana_address::Address;

mod fixtures;

use fixtures::spl::{
    BALANCE_ABOVE_MINIMUM, DECIMALS, DEPOSIT_FEE, MINT, OTHER_MINT, SWEEP_FEE, account_info,
    associated_token_address, failed_sweep_transaction_response, finalize_sweep,
    finalize_sweep_with_amount, frozen_token_account_data, queue_deposit, queue_deposit_for_caller,
    register_token, run_mint_timer, setup_with_registered_mint, spl_token_registration,
    sweep_queued_deposit, token_account_data, token_account_data_with_amount,
};

/// Queues a deposit, sweeps it on the timers, settles it and mints it on the ledger.
#[tokio::test]
async fn should_sweep_finalize_and_mint_queued_deposit_after_timers() {
    let setup = SetupBuilder::new().with_proxy_canister().build().await;
    setup.fund_minter().await;
    setup.top_up_operating_balance().await;
    let ledger = setup.install_spl_ledger(DECIMALS).await;
    register_token(&setup, spl_token_registration(ledger.canister_id())).await;
    let account = Account {
        owner: setup.proxy_canister_id(),
        subaccount: Some([1; 32]),
    };
    let owner: Address = setup.minter().get_deposit_address(account).await.into();
    let deposit_id = queue_deposit(&setup, account, 32).await;
    assert_eq!(ledger.balance_of(account).await, 0);
    assert_eq!(
        setup.minter().deposit_spl_status(deposit_id).await,
        DepositSplStatus::Queued {
            balance: BALANCE_ABOVE_MINIMUM,
            fee: DEPOSIT_FEE,
        }
    );
    assert_eq!(
        setup
            .minter()
            .get_minter_info()
            .await
            .collected_spl_deposit_fees,
        Some(vec![CollectedSplDepositFees {
            mint: MINT.into(),
            amount: 0,
        }])
    );
    assert_eq!(
        setup.minter().deposit_spl_status(deposit_id + 1).await,
        DepositSplStatus::NotFound
    );

    let signature = sweep_queued_deposit(&setup, deposit_id, 36).await;
    assert_eq!(ledger.balance_of(account).await, 0);
    assert_eq!(
        setup.minter().deposit_spl_status(deposit_id).await,
        DepositSplStatus::Swept {
            signature: signature.clone()
        }
    );

    finalize_sweep(&setup, &signature, &owner, 48).await;
    assert_eq!(
        setup.minter().deposit_spl_status(deposit_id).await,
        DepositSplStatus::Finalized {
            signature: signature.clone()
        }
    );
    // Settlement returned the reservation minus the fee that Main paid.
    assert_eq!(
        setup.minter().get_minter_info().await.balance,
        Setup::FUNDED_BALANCE - SWEEP_FEE
    );
    assert_eq!(
        setup.minter().get_operating_balance().await.balance,
        Setup::OPERATING_TOP_UP - SWEEP_FEE
    );
    setup.minter().assert_that_events().await.satisfy(|events| {
        check!(events.contains(&EventType::CreditedSplSweep {
            signature: signature.clone(),
            lamports_spent: SWEEP_FEE,
            deposit_id,
            amount_to_mint: BALANCE_ABOVE_MINIMUM - DEPOSIT_FEE,
        }));
    });
    assert_eq!(
        setup
            .minter()
            .get_minter_info()
            .await
            .collected_spl_deposit_fees,
        Some(vec![CollectedSplDepositFees {
            mint: MINT.into(),
            amount: u128::from(DEPOSIT_FEE),
        }])
    );

    run_mint_timer(&setup).await;
    assert_eq!(
        ledger.balance_of(account).await,
        BALANCE_ABOVE_MINIMUM - DEPOSIT_FEE
    );
    assert_eq!(
        setup.minter().deposit_spl_status(deposit_id).await,
        DepositSplStatus::Minted {
            block_index: 0,
            minted_amount: BALANCE_ABOVE_MINIMUM - DEPOSIT_FEE,
        }
    );
    assert_eq!(
        ledger
            .balance_of(Account {
                subaccount: None,
                ..account
            })
            .await,
        0
    );
    setup.minter().assert_that_events().await.satisfy(|events| {
        // The first mint on this ledger has block index zero.
        check!(events.contains(&EventType::MintedSweptSplDeposit {
            deposit_id,
            mint_block_index: 0
        }));
        check!(!events.iter().any(|event| matches!(
            event,
            EventType::QuarantinedSplSweep { .. } | EventType::QuarantinedPendingSplMint { .. }
        )));
    });
    let memo: Memo = minicbor::decode(&get_memo(ledger.get_block(0).await)).unwrap();
    assert_eq!(
        memo,
        Memo::from(MintMemo::sweep(signature.clone(), deposit_id))
    );
    // The mint released the pair, so the next balance queues as a new deposit.
    assert_eq!(queue_deposit(&setup, account, 64).await, deposit_id + 1);
    assert_matches!(
        setup.minter().deposit_spl_status(deposit_id).await,
        DepositSplStatus::Minted { .. }
    );
    setup.drop().await;
}

#[tokio::test]
async fn should_mint_the_configured_fixed_spl_deposit_fee() {
    const DEPOSIT_BALANCE: u64 = 10_000_000;
    const EXPECTED_FEE: u64 = 1_800;

    let setup = SetupBuilder::new().with_proxy_canister().build().await;
    setup.fund_minter().await;
    setup.top_up_operating_balance().await;
    let ledger = setup.install_spl_ledger(DECIMALS).await;
    register_token(
        &setup,
        SplTokenRegistration {
            deposit_fee: EXPECTED_FEE,
            ..spl_token_registration(ledger.canister_id())
        },
    )
    .await;
    let account = Account {
        owner: setup.proxy_canister_id(),
        subaccount: Some([1; 32]),
    };
    let owner: Address = setup.minter().get_deposit_address(account).await.into();
    let deposit_id = setup
        .minter()
        .with_http_mocks(
            MockBuilder::with_start_id(32)
                .get_account_info(
                    &associated_token_address(&owner),
                    Some(account_info(token_account_data_with_amount(
                        &owner,
                        DEPOSIT_BALANCE,
                    ))),
                )
                .build(),
        )
        .deposit_spl(DepositSplArgs {
            owner: None,
            subaccount: account.subaccount,
            mint: MINT.into(),
        })
        .await
        .expect("deposit_spl should apply the fixed fee and queue the deposit");
    assert_eq!(
        setup.minter().deposit_spl_status(deposit_id).await,
        DepositSplStatus::Queued {
            balance: DEPOSIT_BALANCE,
            fee: EXPECTED_FEE,
        }
    );

    let signature = sweep_queued_deposit(&setup, deposit_id, 36).await;
    finalize_sweep_with_amount(&setup, &signature, &owner, DEPOSIT_BALANCE, 48).await;

    run_mint_timer(&setup).await;
    assert_eq!(
        ledger.balance_of(account).await,
        DEPOSIT_BALANCE - EXPECTED_FEE
    );
    assert_eq!(
        setup.minter().deposit_spl_status(deposit_id).await,
        DepositSplStatus::Minted {
            block_index: 0,
            minted_amount: DEPOSIT_BALANCE - EXPECTED_FEE,
        }
    );
    setup.drop().await;
}

#[tokio::test]
async fn should_sweep_spl_deposits_only_after_an_operating_top_up() {
    let setup = SetupBuilder::new().with_proxy_canister().build().await;
    setup.fund_minter().await;
    let ledger_id = setup.install_spl_ledger(DECIMALS).await.canister_id();
    register_token(&setup, spl_token_registration(ledger_id)).await;
    let account = Account {
        owner: setup.proxy_canister_id(),
        subaccount: Some([1; 32]),
    };
    let deposit_id = queue_deposit(&setup, account, 32).await;

    setup.advance_time_and_settle(SWEEP_DEPOSITS_DELAY).await;
    assert_matches!(
        setup.minter().deposit_spl_status(deposit_id).await,
        DepositSplStatus::Queued { .. }
    );
    assert_eq!(setup.minter().get_operating_balance().await.balance, 0);
    assert_eq!(setup.pending_http_outcalls().await, 0);

    setup.top_up_operating_balance().await;
    let signature = sweep_queued_deposit(&setup, deposit_id, 36).await;
    assert_eq!(
        setup.minter().deposit_spl_status(deposit_id).await,
        DepositSplStatus::Swept { signature }
    );
    setup.drop().await;
}

#[tokio::test]
async fn should_quarantine_deposit_when_finalized_sweep_does_not_match_its_plan() {
    let setup = setup_with_registered_mint().await;
    let account = Account {
        owner: setup.proxy_canister_id(),
        subaccount: Some([1; 32]),
    };
    let owner: Address = setup.minter().get_deposit_address(account).await.into();
    let deposit_id = queue_deposit(&setup, account, 32).await;
    let signature = sweep_queued_deposit(&setup, deposit_id, 36).await;

    finalize_sweep_with_amount(&setup, &signature, &owner, BALANCE_ABOVE_MINIMUM - 1, 48).await;

    setup.minter().assert_that_events().await.satisfy(|events| {
        check!(events.contains(&EventType::QuarantinedSplSweep {
            signature: signature.clone()
        }));
        check!(
            !events
                .iter()
                .any(|event| matches!(event, EventType::CreditedSplSweep { .. }))
        );
    });
    assert_eq!(
        setup.minter().deposit_spl_status(deposit_id).await,
        DepositSplStatus::Quarantined {
            signature: signature.clone()
        }
    );
    // The quarantined sweep keeps its reservation.
    assert_eq!(
        setup.minter().get_minter_info().await.balance,
        Setup::FUNDED_BALANCE - SWEEP_FEE
    );
    assert_eq!(
        setup.minter().get_operating_balance().await.balance,
        Setup::OPERATING_TOP_UP - SWEEP_FEE
    );
    // The pair stays locked, so a new request is rejected with the quarantined deposit.
    assert_eq!(
        setup
            .minter()
            .deposit_spl(DepositSplArgs {
                owner: None,
                subaccount: account.subaccount,
                mint: MINT.into(),
            })
            .await,
        Err(DepositSplError::Quarantined { deposit_id })
    );
    setup.drop().await;
}

#[tokio::test]
async fn should_reject_invalid_deposit_requests() {
    let other_mint: cksol_types::Address = OTHER_MINT.into();
    let setup = SetupBuilder::new().with_proxy_canister().build().await;
    let ledger_id = setup.install_spl_ledger(DECIMALS).await.canister_id();
    register_token(&setup, spl_token_registration(ledger_id)).await;
    let account = Account {
        owner: setup.proxy_canister_id(),
        subaccount: Some([1; 32]),
    };
    let args = |mint: cksol_types::Address| DepositSplArgs {
        owner: None,
        subaccount: account.subaccount,
        mint,
    };

    assert_eq!(
        setup.minter().deposit_spl(args(other_mint.clone())).await,
        Err(DepositSplError::UnsupportedToken {
            mint: other_mint.clone()
        })
    );

    let owner: Address = setup.minter().get_deposit_address(account).await.into();
    let result = setup
        .minter()
        .with_http_mocks(
            MockBuilder::new()
                .get_account_info(
                    &associated_token_address(&owner),
                    Some(account_info(frozen_token_account_data(&owner))),
                )
                .build(),
        )
        .deposit_spl(args(MINT.into()))
        .await;
    assert_matches!(result, Err(DepositSplError::InvalidTokenAccount(reason)) if reason.contains("frozen"));

    // An account that does not exist holds no tokens.
    let result = setup
        .minter()
        .with_http_mocks(
            MockBuilder::with_start_id(4)
                .get_account_info(&associated_token_address(&owner), None)
                .build(),
        )
        .deposit_spl(args(MINT.into()))
        .await;
    assert_eq!(
        result,
        Err(DepositSplError::ValueTooSmall {
            balance: 0,
            minimum_balance: BALANCE_ABOVE_MINIMUM,
        })
    );

    let other_ledger_id = setup.install_spl_ledger(DECIMALS).await.canister_id();
    register_token(
        &setup,
        SplTokenRegistration {
            mint: OTHER_MINT.to_string(),
            deposits_paused: true,
            ..spl_token_registration(other_ledger_id)
        },
    )
    .await;
    assert_eq!(
        setup.minter().deposit_spl(args(other_mint.clone())).await,
        Err(DepositSplError::TokenPaused { mint: other_mint })
    );
    setup.drop().await;
}

#[tokio::test]
async fn should_keep_in_flight_deposit_and_mint_it_after_upgrade() {
    let setup = SetupBuilder::new().with_proxy_canister().build().await;
    setup.fund_minter().await;
    setup.top_up_operating_balance().await;
    let ledger = setup.install_spl_ledger(DECIMALS).await;
    register_token(&setup, spl_token_registration(ledger.canister_id())).await;
    let account = Account {
        owner: setup.proxy_canister_id(),
        subaccount: Some([1; 32]),
    };
    let owner: Address = setup.minter().get_deposit_address(account).await.into();
    let deposit_id = queue_deposit(&setup, account, 32).await;
    let signature = sweep_queued_deposit(&setup, deposit_id, 36).await;
    finalize_sweep(&setup, &signature, &owner, 48).await;
    let events_before_upgrade = setup.minter().get_all_events().await;
    check!(
        events_before_upgrade
            .iter()
            .any(|event| matches!(event.payload, EventType::CreditedSplSweep { .. }))
    );

    setup
        .minter()
        .upgrade(UpgradeArgs::default())
        .await
        .expect("upgrade failed");
    // The init timer fetches the master key again.
    setup.tick().await;

    let events = setup.minter().get_all_events().await;
    assert_eq!(
        events[..events_before_upgrade.len()],
        events_before_upgrade[..]
    );
    assert_eq!(events.len(), events_before_upgrade.len() + 1);
    assert_matches!(events.last().unwrap().payload, EventType::Upgrade(_));
    assert_eq!(
        setup
            .minter()
            .deposit_spl(DepositSplArgs {
                owner: None,
                subaccount: account.subaccount,
                mint: MINT.into(),
            })
            .await,
        Ok(deposit_id)
    );
    assert_eq!(ledger.balance_of(account).await, 0);

    run_mint_timer(&setup).await;
    assert_eq!(
        ledger.balance_of(account).await,
        BALANCE_ABOVE_MINIMUM - DEPOSIT_FEE
    );
    assert_eq!(
        setup.minter().deposit_spl_status(deposit_id).await,
        DepositSplStatus::Minted {
            block_index: 0,
            minted_amount: BALANCE_ABOVE_MINIMUM - DEPOSIT_FEE,
        }
    );
    setup.minter().assert_that_events().await.satisfy(|events| {
        check!(events.contains(&EventType::MintedSweptSplDeposit {
            deposit_id,
            mint_block_index: 0
        }));
    });
    setup.drop().await;
}

#[tokio::test]
async fn should_drop_deposit_and_release_account_after_sweep_expires() {
    let setup = setup_with_registered_mint().await;
    let account = Account {
        owner: setup.proxy_canister_id(),
        subaccount: Some([1; 32]),
    };
    let deposit_id = queue_deposit(&setup, account, 32).await;
    let signature = sweep_queued_deposit(&setup, deposit_id, 36).await;

    setup.advance_time(FINALIZE_TRANSACTIONS_DELAY).await;
    setup
        .execute_http_mocks(
            // getSlot, getBlock and getSignatureStatuses use RPC IDs 48 to 59.
            MockBuilder::with_start_id(48)
                .mark_transaction_expired(&signature, EXPIRY_BLOCK_HEIGHT)
                .build(),
        )
        .await;

    setup.minter().assert_that_events().await.satisfy(|events| {
        check!(events.contains(&EventType::ExpiredTransaction {
            signature: signature.clone()
        }));
        check!(
            !events
                .iter()
                .any(|event| matches!(event, EventType::FailedTransaction { .. }))
        );
    });
    assert_eq!(
        setup.minter().deposit_spl_status(deposit_id).await,
        DepositSplStatus::Dropped {
            signature: signature.clone()
        }
    );
    // A proven expiry costs nothing and releases the entire reservation.
    assert_eq!(
        setup.minter().get_minter_info().await.balance,
        Setup::FUNDED_BALANCE
    );
    assert_eq!(
        setup.minter().get_operating_balance().await.balance,
        Setup::OPERATING_TOP_UP
    );
    assert_eq!(queue_deposit(&setup, account, 60).await, deposit_id + 1);
    setup.drop().await;
}

#[tokio::test]
async fn should_drop_deposit_and_release_account_when_sweep_fails_on_chain() {
    let setup = setup_with_registered_mint().await;
    let account = Account {
        owner: setup.proxy_canister_id(),
        subaccount: Some([1; 32]),
    };
    let owner: Address = setup.minter().get_deposit_address(account).await.into();
    let deposit_id = queue_deposit(&setup, account, 32).await;
    let signature = sweep_queued_deposit(&setup, deposit_id, 36).await;
    let sweep = setup.minter().signed_transaction(&signature).await;

    setup.advance_time(FINALIZE_TRANSACTIONS_DELAY).await;
    setup
        .execute_http_mocks(
            // getSlot, getBlock and getSignatureStatuses use RPC IDs 48 to 59, and the
            // getTransaction that validates the actual cost uses RPC IDs 60 to 63.
            MockBuilder::with_start_id(48)
                .fail_transaction(&signature, SUBMISSION_BLOCK_HEIGHT)
                .get_transaction(
                    &signature,
                    failed_sweep_transaction_response(&sweep, &owner),
                )
                .build(),
        )
        .await;

    setup.minter().assert_that_events().await.satisfy(|events| {
        check!(events.contains(&EventType::FailedTransaction {
            signature: signature.clone()
        }));
        check!(events.contains(&EventType::FailedSplSweep {
            signature: signature.clone(),
            lamports_spent: SWEEP_FEE,
        }));
        check!(
            !events
                .iter()
                .any(|event| matches!(event, EventType::CreditedSplSweep { .. }))
        );
    });
    assert_eq!(
        setup.minter().deposit_spl_status(deposit_id).await,
        DepositSplStatus::Dropped {
            signature: signature.clone()
        }
    );
    // The failed transaction still paid its fee.
    assert_eq!(
        setup.minter().get_minter_info().await.balance,
        Setup::FUNDED_BALANCE - SWEEP_FEE
    );
    assert_eq!(
        setup.minter().get_operating_balance().await.balance,
        Setup::OPERATING_TOP_UP - SWEEP_FEE
    );
    assert_eq!(queue_deposit(&setup, account, 64).await, deposit_id + 1);
    setup.drop().await;
}

#[tokio::test]
async fn should_treat_default_subaccount_spellings_as_one_account() {
    let setup = setup_with_registered_mint().await;
    let deposit_id = queue_deposit(&setup, DEFAULT_CALLER_ACCOUNT, 32).await;

    let result = setup
        .minter()
        .deposit_spl(DepositSplArgs {
            owner: None,
            subaccount: Some([0; 32]),
            mint: MINT.into(),
        })
        .await;

    assert_eq!(result, Ok(deposit_id));
    setup.minter().assert_that_events().await.satisfy(|events| {
        let queued: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                EventType::QueuedSplDeposit { account, .. } => Some(*account),
                _ => None,
            })
            .collect();
        check!(queued == [DEFAULT_CALLER_ACCOUNT]);
    });
    setup.drop().await;
}

#[tokio::test]
async fn should_queue_deposit_for_caller_if_owner_is_omitted() {
    queue_deposit_for_caller().await;
}

#[tokio::test]
async fn should_queue_deposit_for_an_explicit_owner() {
    let setup = SetupBuilder::new().with_proxy_canister().build().await;
    let ledger_id = setup.install_spl_ledger(DECIMALS).await.canister_id();
    register_token(&setup, spl_token_registration(ledger_id)).await;
    let account = Account {
        owner: Principal::from_slice(&[7; 29]),
        subaccount: Some([1; 32]),
    };
    assert_ne!(account.owner, setup.proxy_canister_id());
    let owner: Address = setup.minter().get_deposit_address(account).await.into();
    let source = associated_token_address(&owner);

    let deposit_id = setup
        .minter()
        .with_http_mocks(
            MockBuilder::new()
                .get_account_info(&source, Some(account_info(token_account_data(&owner))))
                .build(),
        )
        .deposit_spl(DepositSplArgs {
            owner: Some(account.owner),
            subaccount: account.subaccount,
            mint: MINT.into(),
        })
        .await
        .expect("deposit_spl should queue a sweep for the given owner");

    setup.minter().assert_that_events().await.satisfy(|events| {
        check!(
            events.last()
                == Some(&EventType::QueuedSplDeposit {
                    deposit_id,
                    account,
                    mint: MINT.into(),
                    address: source.into(),
                    balance: BALANCE_ABOVE_MINIMUM,
                    fee: DEPOSIT_FEE,
                })
        );
    });
    setup.drop().await;
}

mod cycles {
    use super::*;

    /// The default fee the minter keeps for a manually accepted SPL deposit.
    const CONSOLIDATION_FEE: u128 = 70_000_000_000;

    fn deposit_args() -> DepositSplArgs {
        DepositSplArgs {
            owner: None,
            subaccount: None,
            mint: MINT.into(),
        }
    }

    /// The cycles that the SOL RPC canister charges for one account read as the minter sends it.
    async fn get_account_info_cycles_cost(setup: &Setup, address: Address) -> u128 {
        setup
            .sol_rpc()
            .get_account_info(GetAccountInfoParams::from_pubkey(address))
            .with_encoding(GetAccountInfoEncoding::Base64)
            .with_commitment(CommitmentLevel::Finalized)
            .with_rpc_config(RpcConfig {
                response_size_estimate: Some(2_048),
                response_consensus: Some(ConsensusStrategy::Threshold {
                    min: 3,
                    total: Some(4),
                }),
            })
            .request_cost()
            .send()
            .await
            .expect("Failed to get cycles cost for `getAccountInfo` request")
    }

    /// The cycle balances of the caller and of the minter.
    async fn cycle_balances(setup: &Setup) -> (u128, u128) {
        (
            setup.proxy().cycle_balance().await,
            setup.minter().cycle_balance().await,
        )
    }

    #[tokio::test]
    async fn should_charge_the_read_and_the_consolidation_fee() {
        let setup = setup_with_registered_mint().await;
        let owner: Address = DEFAULT_CALLER_DEPOSIT_ADDRESS.parse().unwrap();
        let source = associated_token_address(&owner);
        let read_cost = get_account_info_cycles_cost(&setup, source).await;
        assert!(read_cost > 0);
        let (caller_before, minter_before) = cycle_balances(&setup).await;

        let result = setup
            .minter()
            .with_http_mocks(
                MockBuilder::with_start_id(32)
                    .get_account_info(&source, Some(account_info(token_account_data(&owner))))
                    .build(),
            )
            .deposit_spl(deposit_args())
            .await;

        assert_eq!(result, Ok(0));
        let (caller_after, minter_after) = cycle_balances(&setup).await;
        assert_eq!(caller_before - caller_after, read_cost + CONSOLIDATION_FEE);
        assert_eq!(minter_after - minter_before, CONSOLIDATION_FEE);
        setup.drop().await;
    }

    #[tokio::test]
    async fn should_charge_the_read_without_fee_when_the_balance_is_too_small() {
        let setup = setup_with_registered_mint().await;
        let owner: Address = DEFAULT_CALLER_DEPOSIT_ADDRESS.parse().unwrap();
        let source = associated_token_address(&owner);
        let read_cost = get_account_info_cycles_cost(&setup, source).await;
        let (caller_before, minter_before) = cycle_balances(&setup).await;

        let result = setup
            .minter()
            .with_http_mocks(
                MockBuilder::with_start_id(32)
                    .get_account_info(
                        &source,
                        Some(account_info(token_account_data_with_amount(
                            &owner,
                            BALANCE_ABOVE_MINIMUM - 1,
                        ))),
                    )
                    .build(),
            )
            .deposit_spl(deposit_args())
            .await;

        assert_eq!(
            result,
            Err(DepositSplError::ValueTooSmall {
                balance: BALANCE_ABOVE_MINIMUM - 1,
                minimum_balance: BALANCE_ABOVE_MINIMUM,
            })
        );
        let (caller_after, minter_after) = cycle_balances(&setup).await;
        assert_eq!(caller_before - caller_after, read_cost);
        assert_eq!(minter_after, minter_before);
        setup.drop().await;
    }

    #[tokio::test]
    async fn should_refund_all_cycles_when_too_few_are_attached() {
        let setup = setup_with_registered_mint().await;
        let (caller_before, minter_before) = cycle_balances(&setup).await;

        let result = setup
            .minter()
            .deposit_spl_with_cycles(deposit_args(), CONSOLIDATION_FEE)
            .await;

        assert_matches!(result, Err(DepositSplError::InsufficientCycles(_)));
        assert_eq!(cycle_balances(&setup).await, (caller_before, minter_before));
        setup.drop().await;
    }
}
