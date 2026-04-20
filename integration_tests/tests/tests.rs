use assert_matches::assert_matches;
use assert2::check;
use candid::{Nat, Principal};
use cksol_int_tests::{
    CkSolMinter, Setup, SetupBuilder,
    fixtures::{
        DEFAULT_CALLER_ACCOUNT, DEFAULT_CALLER_DEPOSIT_ADDRESS, DEPOSIT_AMOUNT,
        EXPECTED_MINT_AMOUNT, MockBuilder, SharedMockHttpOutcalls, default_process_deposit_args,
        deposit_transaction_signature,
    },
};
use cksol_types::{
    DepositId, DepositStatus, GetDepositAddressArgs, InsufficientCyclesError, Lamport, MinterInfo,
    ProcessDepositArgs, ProcessDepositError, TxFinalizedStatus, UpdateBalanceArgs, WithdrawalArgs,
    WithdrawalError, WithdrawalStatus,
};
use cksol_types_internal::{
    UpgradeArgs,
    event::{EventType, TransactionPurpose},
    log::Priority,
};
use ic_pocket_canister_runtime::{JsonRpcResponse, MockHttpOutcalls};
use icrc_ledger_types::icrc1::account::{Account, Subaccount};
use serde_json::json;
use sol_rpc_types::{CommitmentLevel, ConsensusStrategy, GetTransactionEncoding, RpcConfig, Slot};
use std::time::Duration;
use tokio::join;

const WITHDRAWAL_PROCESSING_DELAY: Duration = Duration::from_mins(1);
const FINALIZE_TRANSACTIONS_DELAY: Duration = Duration::from_mins(2);
const RESUBMIT_TRANSACTIONS_DELAY: Duration = Duration::from_mins(3);
const DEPOSIT_CONSOLIDATION_DELAY: Duration = Duration::from_mins(10);

/// Deposits funds into the minter via `process_deposit`, consolidates them,
/// and finalizes the consolidation so the minter's internal balance is credited.
///
/// Requires the setup to have been built with `.with_proxy_canister()`.
async fn deposit_and_consolidate_funds(setup: &Setup) {
    let result = setup
        .minter()
        .with_http_mocks(MockBuilder::new().get_deposit_transaction().build())
        .process_deposit(default_process_deposit_args())
        .await;
    assert_matches!(result, Ok(DepositStatus::Minted { .. }));

    // Consolidate
    setup.advance_time(DEPOSIT_CONSOLIDATION_DELAY).await;
    setup
        .execute_http_mocks(
            MockBuilder::with_start_id(4)
                .submit_transaction(
                    100_000_000,
                    "4sGjMW1sUnHzSxGspuhpqLDx6wiyjNtZAMdL4VZHirAn",
                    "5VERv8NMvzbJMEkV8xnrLkEaWRtSz9CosKDYjCJjBRnbJLgp8uirBgmQpjKhoR4tjF3ZpRzrFmBV6UjKdiSZkQUW",
                )
                .build(),
        )
        .await;

    // Finalize
    setup.advance_time(FINALIZE_TRANSACTIONS_DELAY).await;
    setup
        .execute_http_mocks(
            MockBuilder::with_start_id(16)
                .get_current_slot(100_000_000, "4sGjMW1sUnHzSxGspuhpqLDx6wiyjNtZAMdL4VZHirAn")
                .check_signature_statuses_finalized(1)
                .build(),
        )
        .await;
}

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

        // Setting minimum_deposit_amount below automated_deposit_fee should fail
        let result = minter
            .upgrade(UpgradeArgs {
                minimum_deposit_amount: Some(Setup::DEFAULT_AUTOMATED_DEPOSIT_FEE - 1),
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
        const NEW_MANUAL_DEPOSIT_FEE: Lamport = 10;
        const NEW_AUTOMATED_DEPOSIT_FEE: Lamport = 20;
        const NEW_MINIMUM_DEPOSIT_AMOUNT: Lamport = 1_000_000;
        const NEW_WITHDRAWAL_FEE: Lamport = 100_000;
        const NEW_MINIMUM_WITHDRAWAL_AMOUNT: Lamport = 1_000_000;
        const NEW_PROCESS_DEPOSIT_REQUIRED_CYCLES: u128 = 500_000_000_000;

        let setup = SetupBuilder::new().build().await;

        let initial_minter_info = setup.minter().get_minter_info().await;
        assert_eq!(
            initial_minter_info,
            MinterInfo {
                manual_deposit_fee: Setup::DEFAULT_MANUAL_DEPOSIT_FEE,
                automated_deposit_fee: Setup::DEFAULT_AUTOMATED_DEPOSIT_FEE,
                deposit_consolidation_fee: Setup::DEFAULT_DEPOSIT_CONSOLIDATION_FEE,
                minimum_withdrawal_amount: Setup::DEFAULT_MINIMUM_WITHDRAWAL_AMOUNT,
                minimum_deposit_amount: Setup::DEFAULT_MINIMUM_DEPOSIT_AMOUNT,
                withdrawal_fee: Setup::DEFAULT_WITHDRAWAL_FEE,
                process_deposit_required_cycles: Setup::DEFAULT_PROCESS_DEPOSIT_REQUIRED_CYCLES,
                balance: 0,
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
                manual_deposit_fee: Some(NEW_MANUAL_DEPOSIT_FEE),
                automated_deposit_fee: Some(NEW_AUTOMATED_DEPOSIT_FEE),
                minimum_withdrawal_amount: Some(NEW_MINIMUM_WITHDRAWAL_AMOUNT),
                minimum_deposit_amount: Some(NEW_MINIMUM_DEPOSIT_AMOUNT),
                withdrawal_fee: Some(NEW_WITHDRAWAL_FEE),
                process_deposit_required_cycles: Some(NEW_PROCESS_DEPOSIT_REQUIRED_CYCLES as u64),
                deposit_consolidation_fee: None,
            })
            .await
            .expect("upgrade failed");

        let minter_info = setup.minter().get_minter_info().await;
        assert_eq!(
            minter_info,
            MinterInfo {
                manual_deposit_fee: NEW_MANUAL_DEPOSIT_FEE,
                automated_deposit_fee: NEW_AUTOMATED_DEPOSIT_FEE,
                deposit_consolidation_fee: Setup::DEFAULT_DEPOSIT_CONSOLIDATION_FEE,
                minimum_withdrawal_amount: NEW_MINIMUM_WITHDRAWAL_AMOUNT,
                minimum_deposit_amount: NEW_MINIMUM_DEPOSIT_AMOUNT,
                withdrawal_fee: NEW_WITHDRAWAL_FEE,
                process_deposit_required_cycles: NEW_PROCESS_DEPOSIT_REQUIRED_CYCLES,
                balance: 0,
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

mod withdrawal_tests {
    use std::str::FromStr;

    use candid::Nat;
    use cksol_int_tests::{fixtures::get_memo, ledger_init_args::LEDGER_TRANSFER_FEE};
    use cksol_types::{BurnMemo, Memo, WithdrawalOk};
    use cksol_types_internal::UpgradeArgs;
    use icrc_ledger_types::icrc1::account::Account;
    use solana_address::Address;

    use super::*;

    const MAX_BLOCKHASH_AGE: Slot = 150;
    /// The SOL RPC canister rounds the slot returned by getSlot down to the nearest multiple
    /// of this value before querying getBlock and returning the slot to callers.
    const SOL_RPC_SLOT_ROUNDING: u64 = 20;

    #[tokio::test]
    async fn should_validate_solana_address() {
        let setup = SetupBuilder::new().build().await;

        let args = WithdrawalArgs {
            from_subaccount: None,
            amount: u64::MAX,
            address: "InvalidAddress".to_string(),
        };

        let result = setup.minter().withdraw(args).await;
        let err = result.unwrap_err();
        assert_matches!(err, WithdrawalError::MalformedAddress(_));

        let args = WithdrawalArgs {
            from_subaccount: None,
            amount: u64::MAX,
            address: "E4MpwNnMWs2XtW5gVrxZvyS7fMq31QD5HvbxmwP45Tz3".to_string(),
        };

        let result = setup.minter().withdraw(args).await;
        let err = result.unwrap_err();
        assert_eq!(err, WithdrawalError::InsufficientAllowance { allowance: 0 });

        setup.drop().await;
    }

    #[tokio::test]
    async fn should_check_minimum_withdrawal_amount() {
        let setup = SetupBuilder::new().build().await;

        let args = WithdrawalArgs {
            from_subaccount: None,
            amount: Setup::DEFAULT_MINIMUM_WITHDRAWAL_AMOUNT,
            address: "E4MpwNnMWs2XtW5gVrxZvyS7fMq31QD5HvbxmwP45Tz3".to_string(),
        };

        let result = setup.minter().withdraw(args.clone()).await;
        let err = result.unwrap_err();
        assert_eq!(err, WithdrawalError::InsufficientAllowance { allowance: 0 });

        let new_minimum_withdrawal_amount = Setup::DEFAULT_MINIMUM_WITHDRAWAL_AMOUNT + 1;
        setup
            .minter()
            .upgrade(UpgradeArgs {
                minimum_withdrawal_amount: Some(new_minimum_withdrawal_amount),
                ..Default::default()
            })
            .await
            .expect("upgrade failed");

        let result = setup.minter().withdraw(args).await;
        let err = result.unwrap_err();
        assert_eq!(
            err,
            WithdrawalError::ValueTooSmall {
                minimum_withdrawal_amount: new_minimum_withdrawal_amount,
                withdrawal_amount: Setup::DEFAULT_MINIMUM_WITHDRAWAL_AMOUNT,
            }
        );

        let args = WithdrawalArgs {
            from_subaccount: None,
            amount: new_minimum_withdrawal_amount,
            address: "E4MpwNnMWs2XtW5gVrxZvyS7fMq31QD5HvbxmwP45Tz3".to_string(),
        };

        let result = setup.minter().withdraw(args).await;
        let err = result.unwrap_err();
        assert_eq!(err, WithdrawalError::InsufficientAllowance { allowance: 0 });

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
            .withdraw(WithdrawalArgs {
                from_subaccount: None,
                amount: WITHDRAWAL_AMOUNT,
                address: "E4MpwNnMWs2XtW5gVrxZvyS7fMq31QD5HvbxmwP45Tz3".to_string(),
            })
            .await;

        let balance = setup.ledger().balance_of(DEFAULT_CALLER_ACCOUNT).await;
        assert_eq!(balance, WITHDRAWAL_AMOUNT - LEDGER_TRANSFER_FEE);
        assert_eq!(result, Err(WithdrawalError::InsufficientFunds { balance }));

        // Test insufficient allowance
        let result = setup
            .minter()
            .withdraw(WithdrawalArgs {
                from_subaccount: subaccount,
                amount: WITHDRAWAL_AMOUNT,
                address: "E4MpwNnMWs2XtW5gVrxZvyS7fMq31QD5HvbxmwP45Tz3".to_string(),
            })
            .await;

        assert_eq!(
            result,
            Err(WithdrawalError::InsufficientAllowance { allowance: 0 })
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
            .withdraw(WithdrawalArgs {
                from_subaccount: subaccount,
                amount: WITHDRAWAL_AMOUNT,
                address: "E4MpwNnMWs2XtW5gVrxZvyS7fMq31QD5HvbxmwP45Tz3".to_string(),
            })
            .await;

        assert_eq!(
            result,
            Err(WithdrawalError::InsufficientAllowance {
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
            .withdraw(WithdrawalArgs {
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
            .withdraw(WithdrawalArgs {
                from_subaccount: None,
                amount: WITHDRAWAL_AMOUNT,
                address: WITHDRAWAL_ADDRESS.to_string(),
            })
            .await;

        let block_index = result.expect("burn should succeed").block_index;

        let status = setup.minter().withdrawal_status(block_index).await;
        assert_eq!(status, WithdrawalStatus::Pending);
        // 0 is the initial mint block, should be NotFound
        let status = setup.minter().withdrawal_status(0).await;
        assert_eq!(status, WithdrawalStatus::NotFound);
        let status = setup.minter().withdrawal_status(u64::MAX).await;
        assert_eq!(status, WithdrawalStatus::NotFound);

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

        let args = WithdrawalArgs {
            from_subaccount: None,
            amount: WITHDRAWAL_AMOUNT,
            address: WITHDRAWAL_ADDRESS.to_string(),
        };

        let minter1 = setup.minter();
        let minter2 = setup.minter();

        let (result1, result2) = join!(
            minter1.withdraw(args.clone()),
            minter2.withdraw(args.clone()),
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
                .any(|r| matches!(r, Ok(WithdrawalOk { block_index: _ }))),
            "Expected one Minted result, got: {:?}",
            results
        );
        assert!(
            results
                .iter()
                .any(|r| matches!(r, Err(WithdrawalError::AlreadyProcessing))),
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
            .with_proxy_canister()
            .build()
            .await;

        // Deposit and consolidate so the minter has enough balance for withdrawals
        deposit_and_consolidate_funds(&setup).await;

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

        let WithdrawalOk { block_index } = setup
            .minter()
            .withdraw(WithdrawalArgs {
                from_subaccount: None,
                amount: WITHDRAWAL_AMOUNT,
                address: WITHDRAWAL_ADDRESS.to_string(),
            })
            .await
            .expect("withdraw should succeed");

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
        let status = setup.minter().withdrawal_status(block_index).await;
        let original_tx_hash = match &status {
            WithdrawalStatus::TxSent { transaction_id } => transaction_id.clone(),
            other => panic!("Expected TxSent, got: {other:?}"),
        };

        // Advance time to trigger finalize_transactions, which fetches the current slot,
        // checks statuses (not found), and marks the expired transaction for resubmission.
        // The SOL RPC canister rounds the slot down to SOL_RPC_SLOT_ROUNDING before returning
        // it, so we add SOL_RPC_SLOT_ROUNDING + 1 to ensure the rounded slot is strictly
        // greater than INITIAL_SLOT + MAX_BLOCKHASH_AGE (the expiry threshold).
        let resubmission_slot = INITIAL_SLOT + MAX_BLOCKHASH_AGE + SOL_RPC_SLOT_ROUNDING + 1;
        setup.advance_time(FINALIZE_TRANSACTIONS_DELAY).await;
        setup
            .execute_http_mocks(mark_expired_withdrawal_http_mocks(resubmission_slot))
            .await;

        // Advance time to trigger resubmit_transactions. finalize_transactions also
        // fires but has no pending transactions, so it makes no HTTP outcalls.
        setup.advance_time(RESUBMIT_TRANSACTIONS_DELAY).await;
        setup
            .execute_http_mocks(resubmit_withdrawal_http_mocks(resubmission_slot))
            .await;

        // Withdrawal status should now have a different signature
        let status = setup.minter().withdrawal_status(block_index).await;
        let resubmitted_tx_hash = match &status {
            WithdrawalStatus::TxSent { transaction_id } => {
                assert_ne!(
                    *transaction_id, original_tx_hash,
                    "Expected signature to change after resubmission"
                );
                transaction_id.clone()
            }
            other => panic!("Expected TxSent after resubmission, got: {other:?}"),
        };

        // Advance time to trigger finalize_transactions again. This time the
        // transaction is reported as finalized.
        setup.advance_time(FINALIZE_TRANSACTIONS_DELAY).await;
        setup
            .execute_http_mocks(finalize_withdrawal_http_mocks(resubmission_slot))
            .await;

        // Withdrawal status should now be TxFinalized with Success
        let status = setup.minter().withdrawal_status(block_index).await;
        match &status {
            WithdrawalStatus::TxFinalized(TxFinalizedStatus::Success {
                transaction_id, ..
            }) => {
                assert_eq!(
                    *transaction_id, resubmitted_tx_hash,
                    "Expected finalized tx hash to match resubmitted tx hash"
                );
            }
            other => panic!("Expected TxFinalized(Success), got: {other:?}"),
        }

        setup.drop().await;
    }

    fn estimate_blockhash_http_mocks(slot: u64) -> MockHttpOutcalls {
        MockBuilder::with_start_id(28)
            .submit_transaction(
                slot,
                "4sGjMW1sUnHzSxGspuhpqLDx6wiyjNtZAMdL4VZHirAn",
                "drWLXM6bHretgz7KuwvGZvPBeQ8KEbS3AKB2WJPy4TbBDaqdqAiNcj3cTAS7UnyJKM7eEZoUf4DvhY1TKkus9Bp",
            )
            .build()
    }

    /// HTTP mocks for finalize_transactions detecting an expired transaction:
    /// fetches slot, checks status (not found), marks for resubmission.
    fn mark_expired_withdrawal_http_mocks(current_slot: u64) -> MockHttpOutcalls {
        MockBuilder::with_start_id(40)
            .get_current_slot(current_slot, "9ZNTfG4NyQgxy2SWjSiQoUyBPEvXT2xo7fKc5hPYYJ7b")
            .check_signature_statuses_not_found(1)
            .build()
    }

    /// HTTP mocks for resubmit_transactions sending the replacement transaction.
    fn resubmit_withdrawal_http_mocks(current_slot: u64) -> MockHttpOutcalls {
        MockBuilder::with_start_id(52)
            .submit_transaction(
                current_slot,
                "9ZNTfG4NyQgxy2SWjSiQoUyBPEvXT2xo7fKc5hPYYJ7b",
                "5VERv8NMvzbJMEkV8xnrLkEaWRtSz9CosKDYjCJjBRnbJLgp8uirBgmQpjKhoR4tjF3ZpRzrFmBV6UjKdiSZkQUW",
            )
            .build()
    }

    /// HTTP mocks for finalize_transactions confirming the resubmitted transaction.
    fn finalize_withdrawal_http_mocks(current_slot: u64) -> MockHttpOutcalls {
        MockBuilder::with_start_id(64)
            .get_current_slot(current_slot, "9ZNTfG4NyQgxy2SWjSiQoUyBPEvXT2xo7fKc5hPYYJ7b")
            .check_signature_statuses_finalized(1)
            .build()
    }
}

mod process_deposit_tests {
    use super::*;

    #[tokio::test]
    async fn should_fail_with_insufficient_cycles() {
        let setup = SetupBuilder::new().with_proxy_canister().build().await;

        let result = setup
            .minter()
            .process_deposit_with_cycles(
                default_process_deposit_args(),
                Setup::DEFAULT_PROCESS_DEPOSIT_REQUIRED_CYCLES - 1,
            )
            .await;

        assert_eq!(
            result,
            Err(ProcessDepositError::InsufficientCycles(
                InsufficientCyclesError {
                    expected: Setup::DEFAULT_PROCESS_DEPOSIT_REQUIRED_CYCLES,
                    received: Setup::DEFAULT_PROCESS_DEPOSIT_REQUIRED_CYCLES - 1,
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
            .with_http_mocks(
                MockBuilder::new()
                    .get_transaction(transaction_not_found_response())
                    .build(),
            )
            .process_deposit(default_process_deposit_args())
            .await;

        assert_eq!(result, Err(ProcessDepositError::TransactionNotFound));

        setup.drop().await;
    }

    #[tokio::test]
    async fn should_fail_for_concurrent_access() {
        let setup = SetupBuilder::new().with_proxy_canister().build().await;

        // Both minters use the same mocks, whichever gets the guard first will consume them
        let mocks =
            SharedMockHttpOutcalls::new(MockBuilder::new().get_deposit_transaction().build());

        let minter1 = setup.minter().with_http_mocks(mocks.clone());
        let minter2 = setup.minter().with_http_mocks(mocks.clone());

        let (result1, result2) = join!(
            minter1.process_deposit(default_process_deposit_args()),
            minter2.process_deposit(default_process_deposit_args())
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
                .any(|r| matches!(r, Err(ProcessDepositError::AlreadyProcessing))),
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

        // First call to `process_deposit` fails due to minting error
        let first_result = setup
            .minter()
            .with_http_mocks(MockBuilder::new().get_deposit_transaction().build())
            .process_deposit(default_process_deposit_args())
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

        // Second call to `process_deposit` while the ledger is stopped should still return
        // the same status
        let second_result = setup
            .minter()
            .process_deposit(default_process_deposit_args())
            .await;
        assert_eq!(second_result, first_result);

        setup.ledger().start().await;

        // Third call to update balance after re-starting the ledger should result in a
        // successful mint (without making any additional JSON-RPC calls)
        let balance_before = setup.ledger().balance_of(DEFAULT_CALLER_ACCOUNT).await;
        assert_eq!(balance_before, 0);

        let result = setup
            .minter()
            .process_deposit(default_process_deposit_args())
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
    async fn should_process_deposit_only_once_with_same_deposit() {
        let setup = SetupBuilder::new().with_proxy_canister().build().await;

        let balance_before = setup.ledger().balance_of(DEFAULT_CALLER_ACCOUNT).await;
        assert_eq!(balance_before, 0);

        let deposit_signature = deposit_transaction_signature();

        // First call to `process_deposit` should result in mint
        let first_result = setup
            .minter()
            .with_http_mocks(MockBuilder::new().get_deposit_transaction().build())
            .process_deposit(default_process_deposit_args())
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

        // Second call to `process_deposit` should not result in any JSON-RPC calls or mint
        let second_result = setup
            .minter()
            .process_deposit(default_process_deposit_args())
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
        assert!(get_transaction_cycles_cost > 0);

        let caller_cycles_before = setup.proxy().cycle_balance().await;
        let minter_cycles_before = setup.minter().cycle_balance().await;

        let result = setup
            .minter()
            .with_http_mocks(MockBuilder::new().get_deposit_transaction().build())
            .process_deposit(default_process_deposit_args())
            .await;
        assert_matches!(result, Ok(DepositStatus::Minted { .. }));

        let caller_cycles_after = setup.proxy().cycle_balance().await;
        let minter_cycles_after = setup.minter().cycle_balance().await;

        // The caller should be charged the actual cost of the RPC call plus the consolidation fee
        let expected_charge =
            get_transaction_cycles_cost + Setup::DEFAULT_DEPOSIT_CONSOLIDATION_FEE;
        assert_eq!(caller_cycles_before - caller_cycles_after, expected_charge);
        // The minter receives the consolidation fee
        assert_eq!(
            minter_cycles_after - minter_cycles_before,
            Setup::DEFAULT_DEPOSIT_CONSOLIDATION_FEE,
        );

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

mod update_balance_tests {
    use super::*;

    #[tokio::test]
    async fn should_register_accounts_and_be_idempotent() {
        let setup = SetupBuilder::new().build().await;
        let minter = setup.minter();

        // Register the same accounts as used in get_deposit_address_tests
        let accounts = vec![
            Account {
                owner: Setup::DEFAULT_CALLER,
                subaccount: None,
            },
            Account {
                owner: Principal::from_slice(&[1]),
                subaccount: None,
            },
            Account {
                owner: Setup::DEFAULT_CALLER,
                subaccount: Some([1; 32]),
            },
            Account {
                owner: Setup::DEFAULT_CALLER,
                subaccount: Some([2; 32]),
            },
        ];

        for account in &accounts {
            let result = minter
                .update_balance(UpdateBalanceArgs {
                    owner: Some(account.owner),
                    subaccount: account.subaccount,
                })
                .await;
            assert_eq!(result, Ok(()));
        }

        // Calling again for an already-monitored account is idempotent — no new event
        let result = minter
            .update_balance(UpdateBalanceArgs {
                owner: None,
                subaccount: None,
            })
            .await;
        assert_eq!(result, Ok(()));

        // Exactly one StartedMonitoringAccount event per account, no duplicates
        let expected_accounts = accounts.clone();
        minter.assert_that_events().await.satisfy(|events| {
            let monitoring_events: Vec<&Account> = events
                .iter()
                .filter_map(|e| match e {
                    EventType::StartedMonitoringAccount { account } => Some(account),
                    _ => None,
                })
                .collect();
            check!(monitoring_events.len() == expected_accounts.len());
            for account in &expected_accounts {
                check!(monitoring_events.contains(&account));
            }
        });

        setup.drop().await;
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
                })
                .await;
            assert_matches!(result, Err(s) => s.contains("the owner must be non-anonymous"));

            // `process_deposit` endpoint
            let result = minter
                .try_process_deposit(ProcessDepositArgs {
                    owner,
                    subaccount: None,
                    signature: deposit_transaction_signature(),
                })
                .await;
            assert_matches!(result, Err(s) => s.contains("the owner must be non-anonymous"));
        }

        // `withdraw` endpoint (no `owner` field, only anonymous caller applies)
        let minter = setup.minter_with_caller(Principal::anonymous());
        let result = minter
            .try_withdraw(WithdrawalArgs {
                from_subaccount: None,
                amount: Setup::DEFAULT_MINIMUM_WITHDRAWAL_AMOUNT,
                address: "E4MpwNnMWs2XtW5gVrxZvyS7fMq31QD5HvbxmwP45Tz3".to_string(),
            })
            .await;
        assert_matches!(result, Err(s) => s.contains("the owner must be non-anonymous"));

        setup.drop().await;
    }
}

mod consolidation_tests {
    use super::*;

    #[tokio::test]
    async fn should_consolidate_deposits_after_timer() {
        let setup = SetupBuilder::new().with_proxy_canister().build().await;

        let result = setup
            .minter()
            .with_http_mocks(MockBuilder::new().get_deposit_transaction().build())
            .process_deposit(default_process_deposit_args())
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
        MockBuilder::with_start_id(4)
            .submit_transaction(
                100_000_000,
                "4sGjMW1sUnHzSxGspuhpqLDx6wiyjNtZAMdL4VZHirAn",
                "5VERv8NMvzbJMEkV8xnrLkEaWRtSz9CosKDYjCJjBRnbJLgp8uirBgmQpjKhoR4tjF3ZpRzrFmBV6UjKdiSZkQUW",
            )
            .build()
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
            .assert_contains_metric_matching(r#"minter_balance 0 \d+"#)
            .into()
            .drop()
            .await;
    }

    #[tokio::test]
    async fn should_report_post_upgrade_instructions_consumed() {
        let setup = SetupBuilder::new().build().await;

        // After init, the metric should be 0
        let setup = setup
            .check_metrics()
            .await
            .assert_contains_metric_matching(r"post_upgrade_instructions_consumed 0 \d+")
            .into();

        // Perform an upgrade
        setup
            .minter()
            .upgrade(UpgradeArgs::default())
            .await
            .expect("upgrade failed");

        // After upgrade, the metric should be greater than 0
        setup
            .check_metrics()
            .await
            .assert_contains_metric_matching(r"post_upgrade_instructions_consumed [1-9]\d* \d+")
            .into()
            .drop()
            .await;
    }

    #[tokio::test]
    async fn should_report_oldest_incomplete_withdrawal_age() {
        const WITHDRAWAL_AMOUNT: u64 = 100_000_000;
        const WITHDRAWAL_ADDRESS: &str = "E4MpwNnMWs2XtW5gVrxZvyS7fMq31QD5HvbxmwP45Tz3";

        let setup = SetupBuilder::new()
            .with_initial_ledger_balances(vec![(
                DEFAULT_CALLER_ACCOUNT,
                Nat::from(10 * WITHDRAWAL_AMOUNT),
            )])
            .build()
            .await;

        // No incomplete withdrawals: metric should be 0
        let setup = setup
            .check_metrics()
            .await
            .assert_contains_metric_matching(r"oldest_incomplete_withdrawal_age_seconds 0 \d+")
            .into();

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

        setup
            .minter()
            .withdraw(WithdrawalArgs {
                from_subaccount: None,
                amount: WITHDRAWAL_AMOUNT,
                address: WITHDRAWAL_ADDRESS.to_string(),
            })
            .await
            .expect("withdraw should succeed");

        // Advance time by 60 seconds so the age is clearly nonzero
        setup.advance_time(Duration::from_secs(60)).await;
        setup.tick().await;

        setup
            .check_metrics()
            .await
            .assert_contains_metric_matching(r"oldest_incomplete_withdrawal_age_seconds 60 \d+")
            .into()
            .drop()
            .await;
    }
}
