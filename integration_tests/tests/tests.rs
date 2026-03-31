use assert_matches::assert_matches;
use assert2::check;
use candid::Principal;
use cksol_int_tests::{
    CkSolMinter, Setup, SetupBuilder,
    fixtures::{
        DEFAULT_CALLER_ACCOUNT, DEFAULT_CALLER_DEPOSIT_ADDRESS, DEPOSIT_AMOUNT,
        EXPECTED_MINT_AMOUNT, SharedMockHttpOutcalls, default_update_balance_args,
        deposit_transaction_signature, get_block_request, get_block_response,
        get_deposit_transaction_response, get_signature_statuses_not_found_response,
        get_signature_statuses_request, get_slot_request, get_slot_response,
        get_transaction_http_mocks, send_transaction_request, send_transaction_response,
    },
};
use cksol_types::{
    DepositId, DepositStatus, GetDepositAddressArgs, InsufficientCyclesError, Lamport, MinterInfo,
    UpdateBalanceArgs, UpdateBalanceError, WithdrawSolArgs, WithdrawSolError, WithdrawSolStatus,
};
use cksol_types_internal::{
    UpgradeArgs,
    event::{EventType, TransactionPurpose},
    log::Priority,
};
use ic_pocket_canister_runtime::{JsonRpcResponse, MockHttpOutcalls, MockHttpOutcallsBuilder};
use icrc_ledger_types::icrc1::account::Subaccount;
use serde_json::json;
use sol_rpc_types::{CommitmentLevel, ConsensusStrategy, GetTransactionEncoding, RpcConfig, Slot};
use std::time::Duration;
use tokio::join;

mod get_deposit_address_tests {
    use super::*;

    async fn get_deposit_address(
        minter: &CkSolMinter<'_>,
        owner: Option<Principal>,
        subaccount: Option<Subaccount>,
    ) -> String {
        minter
            .get_deposit_address(GetDepositAddressArgs { owner, subaccount })
            .await
            .to_string()
    }

    #[tokio::test]
    async fn should_get_deposit_address() {
        let setup = SetupBuilder::new().build().await;
        let minter = setup.minter();

        // Owner is the default caller
        assert_eq!(
            get_deposit_address(&minter, None, None).await,
            DEFAULT_CALLER_DEPOSIT_ADDRESS
        );

        // Different owner
        assert_eq!(
            get_deposit_address(&minter, Some(Principal::from_slice(&[1])), None).await,
            "Dyh5A77LtkkYan5NJH4vvCji7WJKBQEqCDupPtmUpxoE"
        );

        // Owner is the default caller, but different subaccounts specified
        assert_eq!(
            get_deposit_address(&minter, None, Some([1; 32])).await,
            "92CvpZZ43QjkMFdYzcQceRSdsV9Gkzs3pTwZ2L7Q5R8r"
        );
        assert_eq!(
            get_deposit_address(&minter, None, Some([2; 32])).await,
            "9aordiHmHhaCQVYS8GtKrMdbf5WK6EhYhfyKPyu5S1X3"
        );

        // Caller is anonymous, but we specify the owner explicitly
        let minter = setup.minter_with_caller(Principal::anonymous());
        assert_eq!(
            get_deposit_address(&minter, Some(Setup::DEFAULT_CALLER), None).await,
            DEFAULT_CALLER_DEPOSIT_ADDRESS
        );

        setup.drop().await;
    }
}

mod lifecycle {
    use super::*;

    #[tokio::test]
    async fn should_rollback_if_upgrading_fails() {
        let setup = SetupBuilder::new().build().await;
        let minter = setup.minter();

        let minter_info_before = minter.get_minter_info().await;

        // Setting a deposit fee higher than the minimum deposit amount should fail!
        let result = minter
            .upgrade(UpgradeArgs {
                minimum_deposit_amount: Some(5_000_000),
                deposit_fee: Some(20_000_000),
                ..UpgradeArgs::default()
            })
            .await;
        assert_matches!(result, Err(_));

        let minter_info_after = minter.get_minter_info().await;

        assert_eq!(minter_info_before, minter_info_after);

        setup.drop().await;
    }

    #[tokio::test]
    async fn should_get_logs() {
        let setup = SetupBuilder::new().build().await;

        let logs = setup.minter().retrieve_logs(&Priority::Info).await;

        assert!(logs[0].message.contains("[init]"));

        setup.drop().await;
    }

    #[tokio::test]
    async fn should_get_minter_info_and_upgrade() {
        const NEW_DEPOSIT_FEE: Lamport = 10;
        // minimum_withdrawal_amount must be >= withdrawal_fee + rent exemption threshold (890,880 lamports)
        const NEW_WITHDRAWAL_FEE: Lamport = 100_000;
        const NEW_MINIMUM_WITHDRAWAL_AMOUNT: Lamport = 1_000_000;
        const NEW_MINIMUM_DEPOSIT_AMOUNT: Lamport = 25;
        const NEW_UPDATE_BALANCE_REQUIRED_CYCLES: u128 = 500_000_000_000;

        let setup = SetupBuilder::new().build().await;

        let initial_minter_info = setup.minter().get_minter_info().await;
        assert_eq!(
            initial_minter_info,
            MinterInfo {
                deposit_fee: Setup::DEFAULT_DEPOSIT_FEE,
                minimum_withdrawal_amount: Setup::DEFAULT_MINIMUM_WITHDRAWAL_AMOUNT,
                minimum_deposit_amount: Setup::DEFAULT_MINIMUM_DEPOSIT_AMOUNT,
                withdrawal_fee: Setup::DEFAULT_WITHDRAWAL_FEE,
                update_balance_required_cycles: Setup::DEFAULT_UPDATE_BALANCE_REQUIRED_CYCLES
            }
        );

        // Upgrade with default args should not change any values
        setup
            .minter()
            .upgrade(UpgradeArgs::default())
            .await
            .expect("upgrade failed");

        let minter_info = setup.minter().get_minter_info().await;
        assert_eq!(minter_info, initial_minter_info);

        // Update with non-default upgrade args should update to the specified values
        setup
            .minter()
            .upgrade(UpgradeArgs {
                sol_rpc_canister_id: None,
                deposit_fee: Some(NEW_DEPOSIT_FEE),
                minimum_withdrawal_amount: Some(NEW_MINIMUM_WITHDRAWAL_AMOUNT),
                minimum_deposit_amount: Some(NEW_MINIMUM_DEPOSIT_AMOUNT),
                withdrawal_fee: Some(NEW_WITHDRAWAL_FEE),
                update_balance_required_cycles: Some(NEW_UPDATE_BALANCE_REQUIRED_CYCLES as u64),
            })
            .await
            .expect("upgrade failed");

        let minter_info = setup.minter().get_minter_info().await;
        assert_eq!(
            minter_info,
            MinterInfo {
                deposit_fee: NEW_DEPOSIT_FEE,
                minimum_withdrawal_amount: NEW_MINIMUM_WITHDRAWAL_AMOUNT,
                minimum_deposit_amount: NEW_MINIMUM_DEPOSIT_AMOUNT,
                withdrawal_fee: NEW_WITHDRAWAL_FEE,
                update_balance_required_cycles: NEW_UPDATE_BALANCE_REQUIRED_CYCLES,
            }
        );

        setup.drop().await;
    }

    #[tokio::test]
    async fn should_get_events() {
        let setup = SetupBuilder::new().build().await;
        let minter = setup.minter();

        minter.assert_that_events().await.satisfy(|events| {
            check!(events.len() == 1 && matches!(events[0], EventType::Init(_)));
        });

        minter
            .upgrade(Default::default())
            .await
            .expect("upgrade failed");

        minter.assert_that_events().await.satisfy(|events| {
            check!(events.len() == 2 && matches!(events[1], EventType::Upgrade(_)));
        });

        setup.drop().await;
    }
}

mod withdraw_sol_tests {
    use std::str::FromStr;

    use candid::Nat;
    use cksol_int_tests::{fixtures::get_memo, ledger_init_args::LEDGER_TRANSFER_FEE};
    use cksol_types::{BurnMemo, Memo, WithdrawSolOk};
    use cksol_types_internal::UpgradeArgs;
    use icrc_ledger_types::icrc1::account::Account;
    use solana_address::Address;

    use super::*;

    const WITHDRAWAL_PROCESSING_DELAY: Duration = Duration::from_mins(1);
    const MAX_BLOCKHASH_AGE: Slot = 150;

    #[tokio::test]
    async fn should_validate_solana_address() {
        let setup = SetupBuilder::new().build().await;

        let args = WithdrawSolArgs {
            from_subaccount: None,
            amount: u64::MAX,
            address: "InvalidAddress".to_string(),
        };

        let result = setup.minter().withdraw_sol(args).await;
        let err = result.unwrap_err();
        assert_matches!(err, WithdrawSolError::MalformedAddress(_));

        let args = WithdrawSolArgs {
            from_subaccount: None,
            amount: u64::MAX,
            address: "E4MpwNnMWs2XtW5gVrxZvyS7fMq31QD5HvbxmwP45Tz3".to_string(),
        };

        let result = setup.minter().withdraw_sol(args).await;
        let err = result.unwrap_err();
        assert_eq!(
            err,
            WithdrawSolError::InsufficientAllowance { allowance: 0 }
        );

        setup.drop().await;
    }

    #[tokio::test]
    async fn should_check_minimum_withdrawal_amount() {
        let setup = SetupBuilder::new().build().await;

        let args = WithdrawSolArgs {
            from_subaccount: None,
            amount: Setup::DEFAULT_MINIMUM_WITHDRAWAL_AMOUNT,
            address: "E4MpwNnMWs2XtW5gVrxZvyS7fMq31QD5HvbxmwP45Tz3".to_string(),
        };

        let result = setup.minter().withdraw_sol(args.clone()).await;
        let err = result.unwrap_err();
        assert_eq!(
            err,
            WithdrawSolError::InsufficientAllowance { allowance: 0 }
        );

        let new_minimum_withdrawal_amount = Setup::DEFAULT_MINIMUM_WITHDRAWAL_AMOUNT + 1;
        setup
            .minter()
            .upgrade(UpgradeArgs {
                minimum_withdrawal_amount: Some(new_minimum_withdrawal_amount),
                ..Default::default()
            })
            .await
            .expect("upgrade failed");

        let result = setup.minter().withdraw_sol(args).await;
        let err = result.unwrap_err();
        assert_eq!(
            err,
            WithdrawSolError::AmountTooLow(new_minimum_withdrawal_amount)
        );

        let args = WithdrawSolArgs {
            from_subaccount: None,
            amount: new_minimum_withdrawal_amount,
            address: "E4MpwNnMWs2XtW5gVrxZvyS7fMq31QD5HvbxmwP45Tz3".to_string(),
        };

        let result = setup.minter().withdraw_sol(args).await;
        let err = result.unwrap_err();
        assert_eq!(
            err,
            WithdrawSolError::InsufficientAllowance { allowance: 0 }
        );

        setup.drop().await;
    }

    #[tokio::test]
    async fn should_fail_if_insufficient_funds_or_allowance() {
        const WITHDRAWAL_AMOUNT: u64 = 100_000_000;

        let subaccount = Some([1u8; 32]);
        let caller_account_sub = Account {
            owner: Setup::DEFAULT_CALLER,
            subaccount,
        };

        let setup = SetupBuilder::new()
            .with_initial_ledger_balances(vec![
                (DEFAULT_CALLER_ACCOUNT, Nat::from(WITHDRAWAL_AMOUNT)),
                (caller_account_sub, Nat::from(10 * WITHDRAWAL_AMOUNT)),
            ])
            .build()
            .await;

        // Test insufficent funds
        setup
            .ledger()
            .approve(
                None,
                u64::MAX,
                Account {
                    owner: setup.minter_canister_id(),
                    subaccount: None,
                },
            )
            .await;

        let result = setup
            .minter()
            .withdraw_sol(WithdrawSolArgs {
                from_subaccount: None,
                amount: WITHDRAWAL_AMOUNT,
                address: "E4MpwNnMWs2XtW5gVrxZvyS7fMq31QD5HvbxmwP45Tz3".to_string(),
            })
            .await;

        let balance = setup.ledger().balance_of(DEFAULT_CALLER_ACCOUNT).await;
        assert_eq!(balance, WITHDRAWAL_AMOUNT - LEDGER_TRANSFER_FEE);
        assert_eq!(result, Err(WithdrawSolError::InsufficientFunds { balance }));

        // Test insufficient allowance
        let result = setup
            .minter()
            .withdraw_sol(WithdrawSolArgs {
                from_subaccount: subaccount,
                amount: WITHDRAWAL_AMOUNT,
                address: "E4MpwNnMWs2XtW5gVrxZvyS7fMq31QD5HvbxmwP45Tz3".to_string(),
            })
            .await;

        assert_eq!(
            result,
            Err(WithdrawSolError::InsufficientAllowance { allowance: 0 })
        );

        let approve_amount = WITHDRAWAL_AMOUNT - 1;

        setup
            .ledger()
            .approve(
                subaccount,
                approve_amount,
                Account {
                    owner: setup.minter_canister_id(),
                    subaccount: None,
                },
            )
            .await;

        let result = setup
            .minter()
            .withdraw_sol(WithdrawSolArgs {
                from_subaccount: subaccount,
                amount: WITHDRAWAL_AMOUNT,
                address: "E4MpwNnMWs2XtW5gVrxZvyS7fMq31QD5HvbxmwP45Tz3".to_string(),
            })
            .await;

        assert_eq!(
            result,
            Err(WithdrawSolError::InsufficientAllowance {
                allowance: approve_amount
            })
        );

        let balance = setup.ledger().balance_of(caller_account_sub).await;
        assert_eq!(balance, 10 * WITHDRAWAL_AMOUNT - LEDGER_TRANSFER_FEE);

        setup.drop().await;
    }

    #[tokio::test]
    async fn should_burn_sol_successfully() {
        const WITHDRAWAL_AMOUNT: u64 = 100_000_000;
        const WITHDRAWAL_ADDRESS: &str = "E4MpwNnMWs2XtW5gVrxZvyS7fMq31QD5HvbxmwP45Tz3";

        let initial_balance = 10 * WITHDRAWAL_AMOUNT;

        let setup = SetupBuilder::new()
            .with_initial_ledger_balances(vec![(
                DEFAULT_CALLER_ACCOUNT,
                Nat::from(initial_balance),
            )])
            .build()
            .await;

        setup
            .ledger()
            .approve(
                None,
                WITHDRAWAL_AMOUNT,
                Account {
                    owner: setup.minter_canister_id(),
                    subaccount: None,
                },
            )
            .await;

        let result = setup
            .minter()
            .withdraw_sol(WithdrawSolArgs {
                from_subaccount: None,
                amount: WITHDRAWAL_AMOUNT,
                address: WITHDRAWAL_ADDRESS.to_string(),
            })
            .await;

        let block_index = result.expect("burn should succeed").block_index;

        let block = setup.ledger().get_block(block_index).await;
        let memo_blob = get_memo(block);
        let memo = minicbor::decode::<Memo>(&memo_blob).expect("failed to decode memo");
        let expected_memo = BurnMemo::Convert {
            to_address: Address::from_str(WITHDRAWAL_ADDRESS)
                .expect("failed to decode address")
                .to_bytes(),
        };
        assert_eq!(memo, Memo::from(expected_memo));

        let balance = setup.ledger().balance_of(DEFAULT_CALLER_ACCOUNT).await;
        assert_eq!(
            balance,
            initial_balance - LEDGER_TRANSFER_FEE - WITHDRAWAL_AMOUNT
        );

        setup.drop().await;
    }

    #[tokio::test]
    async fn should_return_withdrawal_status() {
        const WITHDRAWAL_AMOUNT: u64 = 100_000_000;
        const WITHDRAWAL_ADDRESS: &str = "E4MpwNnMWs2XtW5gVrxZvyS7fMq31QD5HvbxmwP45Tz3";

        let initial_balance = 10 * WITHDRAWAL_AMOUNT;

        let setup = SetupBuilder::new()
            .with_initial_ledger_balances(vec![(
                DEFAULT_CALLER_ACCOUNT,
                Nat::from(initial_balance),
            )])
            .build()
            .await;

        setup
            .ledger()
            .approve(
                None,
                WITHDRAWAL_AMOUNT,
                Account {
                    owner: setup.minter_canister_id(),
                    subaccount: None,
                },
            )
            .await;

        let result = setup
            .minter()
            .withdraw_sol(WithdrawSolArgs {
                from_subaccount: None,
                amount: WITHDRAWAL_AMOUNT,
                address: WITHDRAWAL_ADDRESS.to_string(),
            })
            .await;

        let block_index = result.expect("burn should succeed").block_index;

        let status = setup.minter().withdraw_sol_status(block_index).await;
        assert_eq!(status, WithdrawSolStatus::Pending);
        // 0 is the initial mint block, should be NotFound
        let status = setup.minter().withdraw_sol_status(0).await;
        assert_eq!(status, WithdrawSolStatus::NotFound);
        let status = setup.minter().withdraw_sol_status(u64::MAX).await;
        assert_eq!(status, WithdrawSolStatus::NotFound);

        setup.drop().await;
    }

    #[tokio::test]
    async fn should_fail_if_already_processing() {
        const WITHDRAWAL_AMOUNT: u64 = 100_000_000;
        const WITHDRAWAL_ADDRESS: &str = "E4MpwNnMWs2XtW5gVrxZvyS7fMq31QD5HvbxmwP45Tz3";

        let initial_balance = 10 * WITHDRAWAL_AMOUNT;

        let setup = SetupBuilder::new()
            .with_initial_ledger_balances(vec![(
                DEFAULT_CALLER_ACCOUNT,
                Nat::from(initial_balance),
            )])
            .build()
            .await;

        setup
            .ledger()
            .approve(
                None,
                u64::MAX,
                Account {
                    owner: setup.minter_canister_id(),
                    subaccount: None,
                },
            )
            .await;

        let args = WithdrawSolArgs {
            from_subaccount: None,
            amount: WITHDRAWAL_AMOUNT,
            address: WITHDRAWAL_ADDRESS.to_string(),
        };

        let minter1 = setup.minter();
        let minter2 = setup.minter();

        let (result1, result2) = join!(
            minter1.withdraw_sol(args.clone()),
            minter2.withdraw_sol(args.clone()),
        );

        let (result1, result2) = match (&result1, &result2) {
            (Ok(_), Err(_)) => (result1, result2),
            (Err(_), Ok(_)) => (result2, result1),
            _ => panic!("Expected one success and one error, but got: {result1:?} and {result2:?}"),
        };

        // One should succeed, one should fail with AlreadyProcessing (order is non-deterministic)
        let results = [&result1, &result2];
        assert!(
            results
                .iter()
                .any(|r| matches!(r, Ok(WithdrawSolOk { block_index: _ }))),
            "Expected one Minted result, got: {:?}",
            results
        );
        assert!(
            results
                .iter()
                .any(|r| matches!(r, Err(WithdrawSolError::AlreadyProcessing))),
            "Expected one AlreadyProcessing result, got: {:?}",
            results
        );

        setup.drop().await;
    }

    #[tokio::test]
    async fn should_process_pending_withdrawals() {
        const WITHDRAWAL_AMOUNT: u64 = 100_000_000;
        const WITHDRAWAL_ADDRESS: &str = "E4MpwNnMWs2XtW5gVrxZvyS7fMq31QD5HvbxmwP45Tz3";

        let setup = SetupBuilder::new()
            .with_initial_ledger_balances(vec![(
                DEFAULT_CALLER_ACCOUNT,
                Nat::from(10 * WITHDRAWAL_AMOUNT),
            )])
            .build()
            .await;

        setup
            .ledger()
            .approve(
                None,
                WITHDRAWAL_AMOUNT,
                Account {
                    owner: setup.minter_canister_id(),
                    subaccount: None,
                },
            )
            .await;

        let WithdrawSolOk { block_index } = setup
            .minter()
            .withdraw_sol(WithdrawSolArgs {
                from_subaccount: None,
                amount: WITHDRAWAL_AMOUNT,
                address: WITHDRAWAL_ADDRESS.to_string(),
            })
            .await
            .expect("withdraw_sol should succeed");

        const INITIAL_SLOT: u64 = 350_000_000;

        setup.advance_time(WITHDRAWAL_PROCESSING_DELAY).await;
        setup
            .execute_http_mocks(estimate_blockhash_http_mocks(INITIAL_SLOT))
            .await;

        setup.minter().assert_that_events().await.satisfy(|events| {
            check!(events.iter().any(|e| matches!(
                e,
                EventType::SubmittedTransaction {
                    purpose: TransactionPurpose::WithdrawSol { burn_indices },
                    ..
                } if burn_indices == &[block_index]
            )));
        });

        // Withdrawal status should be TxSent with some signature
        let status = setup.minter().withdraw_sol_status(block_index).await;
        let original_tx_hash = match &status {
            WithdrawSolStatus::TxSent(tx) => tx.transaction_hash.clone(),
            other => panic!("Expected TxSent, got: {other:?}"),
        };

        // Advance time to trigger resubmission. The mocked slot exceeds
        // INITIAL_SLOT + MAX_PROCESSING_AGE, so the original transaction
        // is now considered expired.
        const MONITOR_DELAY: Duration = Duration::from_secs(60);
        setup.advance_time(MONITOR_DELAY).await;
        setup
            .execute_http_mocks(resubmit_withdrawal_http_mocks(
                INITIAL_SLOT + MAX_BLOCKHASH_AGE + 50,
            ))
            .await;

        // Withdrawal status should now have a different signature
        let status = setup.minter().withdraw_sol_status(block_index).await;
        match &status {
            WithdrawSolStatus::TxSent(tx) => {
                assert_ne!(
                    tx.transaction_hash, original_tx_hash,
                    "Expected signature to change after resubmission"
                );
            }
            other => panic!("Expected TxSent after resubmission, got: {other:?}"),
        }

        setup.drop().await;
    }

    fn estimate_blockhash_http_mocks(slot: u64) -> MockHttpOutcalls {
        const BLOCKHASH: &str = "4sGjMW1sUnHzSxGspuhpqLDx6wiyjNtZAMdL4VZHirAn";
        const TX_SIGNATURE: &str = "drWLXM6bHretgz7KuwvGZvPBeQ8KEbS3AKB2WJPy4TbBDaqdqAiNcj3cTAS7UnyJKM7eEZoUf4DvhY1TKkus9Bp";

        let mut builder = MockHttpOutcallsBuilder::new();
        // getSlot requests for get_recent_slot_and_blockhash
        for id in 0..4u64 {
            builder = builder
                .given(get_slot_request().with_id(id))
                .respond_with(get_slot_response(slot).with_id(id))
        }
        // getBlock requests for get_recent_slot_and_blockhash
        for id in 4..8u64 {
            builder = builder
                .given(get_block_request(slot).with_id(id))
                .respond_with(get_block_response(BLOCKHASH).with_id(id))
        }
        // sendTransaction (IDs 8-11)
        for id in 8..12u64 {
            builder = builder
                .given(send_transaction_request().with_id(id))
                .respond_with(send_transaction_response(TX_SIGNATURE).with_id(id))
        }
        builder.build()
    }

    /// HTTP mocks for resubmitting an expired withdrawal transaction.
    fn resubmit_withdrawal_http_mocks(current_slot: u64) -> MockHttpOutcalls {
        const NEW_BLOCKHASH: &str = "9ZNTfG4NyQgxy2SWjSiQoUyBPEvXT2xo7fKc5hPYYJ7b";
        const NEW_TX_SIGNATURE: &str = "5VERv8NMvzbJMEkV8xnrLkEaWRtSz9CosKDYjCJjBRnbJLgp8uirBgmQpjKhoR4tjF3ZpRzrFmBV6UjKdiSZkQUW";

        let mut builder = MockHttpOutcallsBuilder::new();
        // getSignatureStatuses (IDs 12-15)
        for id in 12..16u64 {
            builder = builder
                .given(get_signature_statuses_request().with_id(id))
                .respond_with(get_signature_statuses_not_found_response(1).with_id(id))
        }
        // get_recent_slot_and_blockhash for current slot check: getSlot (IDs 16-19)
        for id in 16..20u64 {
            builder = builder
                .given(get_slot_request().with_id(id))
                .respond_with(get_slot_response(current_slot).with_id(id))
        }
        // get_recent_slot_and_blockhash for current slot check: getBlock (IDs 20-23)
        for id in 20..24u64 {
            builder = builder
                .given(get_block_request(current_slot).with_id(id))
                .respond_with(get_block_response(NEW_BLOCKHASH).with_id(id))
        }
        // get_recent_slot_and_blockhash for resubmission: getSlot (IDs 24-27)
        for id in 24..28u64 {
            builder = builder
                .given(get_slot_request().with_id(id))
                .respond_with(get_slot_response(current_slot).with_id(id))
        }
        // get_recent_slot_and_blockhash for resubmission: getBlock (IDs 28-31)
        for id in 28..32u64 {
            builder = builder
                .given(get_block_request(current_slot).with_id(id))
                .respond_with(get_block_response(NEW_BLOCKHASH).with_id(id))
        }
        // sendTransaction (IDs 32-35)
        for id in 32..36u64 {
            builder = builder
                .given(send_transaction_request().with_id(id))
                .respond_with(send_transaction_response(NEW_TX_SIGNATURE).with_id(id))
        }
        builder.build()
    }
}

mod update_balance_tests {
    use super::*;

    #[tokio::test]
    async fn should_fail_with_insufficient_cycles() {
        let setup = SetupBuilder::new().with_proxy_canister().build().await;

        let result = setup
            .minter()
            .update_balance_with_cycles(
                default_update_balance_args(),
                Setup::DEFAULT_UPDATE_BALANCE_REQUIRED_CYCLES - 1,
            )
            .await;

        assert_eq!(
            result,
            Err(UpdateBalanceError::InsufficientCycles(
                InsufficientCyclesError {
                    expected: Setup::DEFAULT_UPDATE_BALANCE_REQUIRED_CYCLES,
                    received: Setup::DEFAULT_UPDATE_BALANCE_REQUIRED_CYCLES - 1,
                }
            ))
        );

        setup.drop().await;
    }

    #[tokio::test]
    async fn should_fail_if_transaction_not_found() {
        fn transaction_not_found_response() -> JsonRpcResponse {
            JsonRpcResponse::from(json!({"jsonrpc": "2.0", "result": null, "id": 0}))
        }

        let setup = SetupBuilder::new().with_proxy_canister().build().await;

        let result = setup
            .minter()
            .with_http_mocks(get_transaction_http_mocks(transaction_not_found_response))
            .update_balance(default_update_balance_args())
            .await;

        assert_eq!(result, Err(UpdateBalanceError::TransactionNotFound));

        setup.drop().await;
    }

    #[tokio::test]
    async fn should_fail_for_concurrent_access() {
        let setup = SetupBuilder::new().with_proxy_canister().build().await;

        // Both minters use the same mocks, whichever gets the guard first will consume them
        let mocks = SharedMockHttpOutcalls::new(get_transaction_http_mocks(
            get_deposit_transaction_response,
        ));

        let minter1 = setup.minter().with_http_mocks(mocks.clone());
        let minter2 = setup.minter().with_http_mocks(mocks.clone());

        let (result1, result2) = join!(
            minter1.update_balance(default_update_balance_args()),
            minter2.update_balance(default_update_balance_args())
        );

        let (result1, result2) = match (&result1, &result2) {
            (Ok(_), Err(_)) => (result1, result2),
            (Err(_), Ok(_)) => (result2, result1),
            _ => panic!("Expected one success and one error, but got: {result1:?} and {result2:?}"),
        };

        // One should succeed, one should fail with `AlreadyProcessing` (order is non-deterministic)
        let results = [&result1, &result2];
        assert!(
            results
                .iter()
                .any(|r| matches!(r, Ok(DepositStatus::Minted { .. }))),
            "Expected one Minted result, got: {:?}",
            results
        );
        assert!(
            results
                .iter()
                .any(|r| matches!(r, Err(UpdateBalanceError::AlreadyProcessing))),
            "Expected one AlreadyProcessing result, got: {:?}",
            results
        );

        setup.drop().await;
    }

    #[tokio::test]
    async fn should_return_processing_if_minting_fails_and_mint_on_retry() {
        let setup = SetupBuilder::new().with_proxy_canister().build().await;

        setup.ledger().stop().await;

        let deposit_signature = deposit_transaction_signature();

        // First call to `update_balance` fails due to minting error
        let first_result = setup
            .minter()
            .with_http_mocks(get_transaction_http_mocks(get_deposit_transaction_response))
            .update_balance(default_update_balance_args())
            .await;
        assert_eq!(
            first_result,
            Ok(DepositStatus::Processing {
                deposit_amount: DEPOSIT_AMOUNT,
                amount_to_mint: EXPECTED_MINT_AMOUNT,
                deposit_id: DepositId {
                    signature: deposit_signature.clone(),
                    account: DEFAULT_CALLER_ACCOUNT,
                },
            })
        );

        // Second call to `update_balance` while the ledger is stopped should still return
        // the same status
        let second_result = setup
            .minter()
            .update_balance(default_update_balance_args())
            .await;
        assert_eq!(second_result, first_result);

        setup.ledger().start().await;

        // Third call to update balance after re-starting the ledger should result in a
        // successful mint (without making any additional JSON-RPC calls)
        let balance_before = setup.ledger().balance_of(DEFAULT_CALLER_ACCOUNT).await;
        assert_eq!(balance_before, 0);

        let result = setup
            .minter()
            .update_balance(default_update_balance_args())
            .await;
        assert_matches!(&result, Ok(DepositStatus::Minted {
            minted_amount,
            deposit_id,
            block_index: _,
        }) if minted_amount == &EXPECTED_MINT_AMOUNT
            && deposit_id.signature == deposit_signature
            && deposit_id.account == DEFAULT_CALLER_ACCOUNT);

        let balance_after = setup.ledger().balance_of(DEFAULT_CALLER_ACCOUNT).await;
        assert_eq!(balance_after, EXPECTED_MINT_AMOUNT);

        setup.drop().await;
    }

    #[tokio::test]
    async fn should_update_balance_only_once_with_same_deposit() {
        let setup = SetupBuilder::new().with_proxy_canister().build().await;

        let balance_before = setup.ledger().balance_of(DEFAULT_CALLER_ACCOUNT).await;
        assert_eq!(balance_before, 0);

        let deposit_signature = deposit_transaction_signature();

        // First call to `update_balance` should result in mint
        let first_result = setup
            .minter()
            .with_http_mocks(get_transaction_http_mocks(get_deposit_transaction_response))
            .update_balance(default_update_balance_args())
            .await;
        assert_matches!(&first_result, Ok(DepositStatus::Minted {
            minted_amount,
            deposit_id,
            block_index: _,
        }) if minted_amount == &EXPECTED_MINT_AMOUNT
            && deposit_id.signature == deposit_signature
            && deposit_id.account == DEFAULT_CALLER_ACCOUNT);

        let balance_after = setup.ledger().balance_of(DEFAULT_CALLER_ACCOUNT).await;
        assert_eq!(balance_after, EXPECTED_MINT_AMOUNT);

        // Second call to `update_balance` should not result in any JSON-RPC calls or mint
        let second_result = setup
            .minter()
            .update_balance(default_update_balance_args())
            .await;
        assert_eq!(second_result, first_result);

        let balance_after = setup.ledger().balance_of(DEFAULT_CALLER_ACCOUNT).await;
        assert_eq!(balance_after, EXPECTED_MINT_AMOUNT);

        setup.drop().await;
    }

    #[tokio::test]
    async fn should_refund_extra_cycles() {
        let setup = SetupBuilder::new().with_proxy_canister().build().await;

        let get_transaction_cycles_cost = get_transaction_cycles_cost(&setup).await;

        let caller_cycles_before = setup.proxy().cycle_balance().await;
        let minter_cycles_before = setup.minter().cycle_balance().await;

        let result = setup
            .minter()
            .with_http_mocks(get_transaction_http_mocks(get_deposit_transaction_response))
            .update_balance(default_update_balance_args())
            .await;
        assert_matches!(result, Ok(DepositStatus::Minted { .. }));

        let caller_cycles_after = setup.proxy().cycle_balance().await;
        let minter_cycles_after = setup.minter().cycle_balance().await;

        // The caller should be charged only the actual cost of making the RPC call
        assert!(get_transaction_cycles_cost > 0);
        assert_eq!(
            caller_cycles_before - caller_cycles_after,
            get_transaction_cycles_cost,
        );
        assert_eq!(minter_cycles_after, minter_cycles_before);

        setup.drop().await;
    }

    async fn get_transaction_cycles_cost(setup: &Setup) -> u128 {
        setup
            .sol_rpc()
            .get_transaction(solana_signature::Signature::from(
                deposit_transaction_signature(),
            ))
            .with_rpc_config(RpcConfig {
                response_size_estimate: Some(2_000_000),
                response_consensus: Some(ConsensusStrategy::Threshold {
                    min: 3,
                    total: Some(4),
                }),
            })
            .with_encoding(GetTransactionEncoding::Base64)
            .with_commitment(CommitmentLevel::Finalized)
            .request_cost()
            .send()
            .await
            .expect("Failed to get cycles cost for `getTransaction` request")
    }
}

mod anonymous_caller_tests {
    use super::*;

    #[tokio::test]
    async fn should_fail_for_anonymous_owner() {
        let setup = SetupBuilder::new().build().await;

        for (caller, owner) in [
            // Caller is default caller, but the owner is specified explicitly to anonymous
            (Setup::DEFAULT_CALLER, Some(Principal::anonymous())),
            // Anonymous caller and owner not specified
            (Principal::anonymous(), None),
        ] {
            let minter = setup.minter_with_caller(caller);

            // `get_deposit_address` endpoint
            let result = minter
                .try_get_deposit_address(GetDepositAddressArgs {
                    owner,
                    subaccount: None,
                })
                .await;
            assert_matches!(result, Err(s) => s.contains("the owner must be non-anonymous"));

            // `update_balance` endpoint
            let result = minter
                .try_update_balance(UpdateBalanceArgs {
                    owner,
                    subaccount: None,
                    signature: deposit_transaction_signature(),
                })
                .await;
            assert_matches!(result, Err(s) => s.contains("the owner must be non-anonymous"));
        }

        setup.drop().await;
    }
}

mod consolidation_tests {
    use super::*;

    const DEPOSIT_CONSOLIDATION_DELAY: Duration = Duration::from_secs(600);

    #[tokio::test]
    async fn should_consolidate_deposits_after_timer() {
        let setup = SetupBuilder::new().with_proxy_canister().build().await;

        let result = setup
            .minter()
            .with_http_mocks(get_transaction_http_mocks(get_deposit_transaction_response))
            .update_balance(default_update_balance_args())
            .await;
        let mint_block_index =
            assert_matches!(result, Ok(DepositStatus::Minted { block_index, .. }) => block_index);

        // Advance time past the consolidation delay to trigger the timer
        setup.advance_time(DEPOSIT_CONSOLIDATION_DELAY).await;
        setup
            .execute_http_mocks(http_mocks_for_deposit_consolidation())
            .await;

        // Verify consolidation events were recorded
        let events_after = setup.minter().get_all_events().await;
        check!(events_after.iter().any(|e| matches!(
            &e.payload,
            EventType::SubmittedTransaction {
                purpose: TransactionPurpose::ConsolidateDeposits { mint_indices },
                ..
            } if mint_indices == &[mint_block_index]
        )));

        setup.drop().await;
    }

    // Returns the required HTTP outcall mocks for executing the deposit consolidation task
    fn http_mocks_for_deposit_consolidation() -> MockHttpOutcalls {
        const SLOT: u64 = 100_000_000;
        const BLOCKHASH: &str = "4sGjMW1sUnHzSxGspuhpqLDx6wiyjNtZAMdL4VZHirAn";
        const TX_SIGNATURE: &str = "5VERv8NMvzbJMEkV8xnrLkEaWRtSz9CosKDYjCJjBRnbJLgp8uirBgmQpjKhoR4tjF3ZpRzrFmBV6UjKdiSZkQUW";

        let mut mocks = MockHttpOutcallsBuilder::new();
        // getSlot requests for get_recent_slot_and_blockhash (IDs 4-7)
        for id in 4..8 {
            mocks = mocks
                .given(get_slot_request().with_id(id))
                .respond_with(get_slot_response(SLOT).with_id(id));
        }
        // getBlock requests for get_recent_slot_and_blockhash (IDs 8-11)
        for id in 8..12 {
            mocks = mocks
                .given(get_block_request(SLOT).with_id(id))
                .respond_with(get_block_response(BLOCKHASH).with_id(id));
        }
        // sendTransaction requests (IDs 12-15)
        for id in 12..16 {
            mocks = mocks
                .given(send_transaction_request().with_id(id))
                .respond_with(send_transaction_response(TX_SIGNATURE).with_id(id));
        }
        mocks.build()
    }
}

mod metrics_tests {
    use super::*;

    #[tokio::test]
    async fn should_serve_metrics() {
        let setup = SetupBuilder::new().build().await;

        setup
            .check_metrics()
            .await
            .assert_contains_metric_matching(r#"stable_memory_bytes \d+ \d+"#)
            .assert_contains_metric_matching(r#"heap_memory_bytes \d+ \d+"#)
            .assert_contains_metric_matching(r#"cycle_balance\{canister="cksol-minter"\} \d+ \d+"#)
            // Only the canister init event should have been recorded
            .assert_contains_metric_matching(r#"total_event_count 1 \d+"#)
            .into()
            .drop()
            .await;
    }
}
