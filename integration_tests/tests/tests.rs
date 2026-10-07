use assert_matches::assert_matches;
use assert2::check;
use candid::{Nat, Principal};
use cksol_int_tests::{
    CkSolMinter, Setup, SetupBuilder,
    fixtures::{
        DEFAULT_CALLER_ACCOUNT, DEFAULT_CALLER_DEPOSIT_ADDRESS, DEPOSIT_AMOUNT, MINTER_ADDRESS,
        MockBuilder, RENT_EXEMPTION_THRESHOLD, SharedMockHttpOutcalls,
    },
    validator::FEE_PER_SIGNATURE,
};
use cksol_types::{
    DepositSolArgs, DepositSolError, DepositSolStatus, GetDepositAddressArgs, Lamport, MinterInfo,
    TxFinalizedStatus, WithdrawalArgs, WithdrawalError, WithdrawalStatus,
};
use cksol_types_internal::{
    UpgradeArgs,
    event::{EventType, TransactionPurpose},
    log::Priority,
};
use icrc_ledger_types::icrc1::account::{Account, Subaccount};
use sol_rpc_types::{CommitmentLevel, ConsensusStrategy, RpcConfig};
use std::time::Duration;
use tokio::join;

const WITHDRAWAL_PROCESSING_DELAY: Duration = Duration::from_mins(1);
const FINALIZE_TRANSACTIONS_DELAY: Duration = Duration::from_mins(2);
const SWEEP_DEPOSITS_DELAY: Duration = Duration::from_mins(1);
/// Number of blocks a blockhash stays valid for, as the minter counts them.
const MAX_BLOCKHASH_AGE_IN_BLOCKS: u64 = 150;
/// Height of the block whose blockhash the mocks hand a timer for a first submission.
const SUBMISSION_BLOCK_HEIGHT: u64 = 100_000_000;
/// First height at which the blockhash of a transaction submitted at
/// [`SUBMISSION_BLOCK_HEIGHT`] is no longer accepted.
const EXPIRY_BLOCK_HEIGHT: u64 = SUBMISSION_BLOCK_HEIGHT + MAX_BLOCKHASH_AGE_IN_BLOCKS + 1;

/// Deposits funds into the minter via `deposit_sol`, sweeps them, and finalizes
/// and credits the sweep so the minter's internal balance is credited.
///
/// Requires the setup to have been built with `.with_proxy_canister()`.
async fn deposit_and_credit_funds(setup: &Setup) {
    const FUNDING_BALANCE: Lamport = DEPOSIT_AMOUNT + RENT_EXEMPTION_THRESHOLD;
    const SWEEPABLE_AMOUNT: Lamport = DEPOSIT_AMOUNT;

    let deposit_id = setup
        .minter()
        .with_http_mocks(MockBuilder::new().get_balance(FUNDING_BALANCE).build())
        .deposit_sol(DEFAULT_CALLER_ACCOUNT)
        .await
        .expect("deposit_sol should queue a sweep");

    // Sweep
    setup.advance_time(SWEEP_DEPOSITS_DELAY).await;
    setup
        .execute_http_mocks(
            MockBuilder::with_start_id(4)
                .submit_transaction(SUBMISSION_BLOCK_HEIGHT)
                .build(),
        )
        .await;
    let sweep_signature = assert_matches!(
        setup.minter().deposit_status(deposit_id).await,
        DepositSolStatus::Swept { signature } => signature
    );

    // Finalize and credit
    setup.advance_time(FINALIZE_TRANSACTIONS_DELAY).await;
    let sweep = setup.minter().signed_transaction(&sweep_signature).await;
    setup
        .execute_http_mocks(
            MockBuilder::with_start_id(16)
                .finalize_transaction(&sweep_signature, SUBMISSION_BLOCK_HEIGHT)
                .get_sweep_transaction(&sweep, SWEEPABLE_AMOUNT)
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

        // A minimum deposit amount below the fee of a full sweep should fail
        let result = minter
            .upgrade(UpgradeArgs {
                minimum_deposit_amount: Some(1),
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
        const NEW_MINIMUM_DEPOSIT_AMOUNT: Lamport = 2_000_000;
        const NEW_WITHDRAWAL_FEE: Lamport = 100_000;
        const NEW_MINIMUM_WITHDRAWAL_AMOUNT: Lamport = 1_000_000;
        const NEW_DEPOSIT_SOL_REQUIRED_CYCLES: u128 = 500_000_000_000;

        let setup = SetupBuilder::new().build().await;
        wait_for_initial_minter_address(&setup).await;

        let initial_minter_info = setup.minter().get_minter_info().await;
        assert_eq!(
            initial_minter_info,
            MinterInfo {
                deposit_sol_fee: Setup::DEFAULT_DEPOSIT_SOL_FEE,
                minimum_withdrawal_amount: Setup::DEFAULT_MINIMUM_WITHDRAWAL_AMOUNT,
                minimum_deposit_amount: Setup::DEFAULT_MINIMUM_DEPOSIT_AMOUNT,
                withdrawal_fee: Setup::DEFAULT_WITHDRAWAL_FEE,
                deposit_sol_required_cycles: Setup::DEFAULT_DEPOSIT_SOL_REQUIRED_CYCLES,
                balance: 0,
                minter_address: Some(MINTER_ADDRESS.to_string()),
                nonce_accounts: vec![Setup::DEFAULT_NONCE_ACCOUNT.to_string()],
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
                minimum_withdrawal_amount: Some(NEW_MINIMUM_WITHDRAWAL_AMOUNT),
                minimum_deposit_amount: Some(NEW_MINIMUM_DEPOSIT_AMOUNT),
                withdrawal_fee: Some(NEW_WITHDRAWAL_FEE),
                deposit_sol_required_cycles: Some(NEW_DEPOSIT_SOL_REQUIRED_CYCLES as u64),
                deposit_sol_fee: None,
                nonce_accounts_to_add: None,
            })
            .await
            .expect("upgrade failed");

        let minter_info = setup.minter().get_minter_info().await;
        assert_eq!(
            minter_info,
            MinterInfo {
                deposit_sol_fee: Setup::DEFAULT_DEPOSIT_SOL_FEE,
                minimum_withdrawal_amount: NEW_MINIMUM_WITHDRAWAL_AMOUNT,
                minimum_deposit_amount: NEW_MINIMUM_DEPOSIT_AMOUNT,
                withdrawal_fee: NEW_WITHDRAWAL_FEE,
                deposit_sol_required_cycles: NEW_DEPOSIT_SOL_REQUIRED_CYCLES,
                balance: 0,
                minter_address: Some(MINTER_ADDRESS.to_string()),
                nonce_accounts: vec![Setup::DEFAULT_NONCE_ACCOUNT.to_string()],
            }
        );

        setup.drop().await;
    }

    #[tokio::test]
    async fn should_get_events() {
        let setup = SetupBuilder::new().build().await;
        let minter = setup.minter();

        minter.assert_that_events().await.satisfy(|events| {
            check!(matches!(events[0], EventType::Init(_)));
            check!(
                events[1..]
                    .iter()
                    .all(|e| matches!(e, EventType::MinterPublicKeyFetched { .. }))
            );
        });

        minter
            .upgrade(Default::default())
            .await
            .expect("upgrade failed");

        minter.assert_that_events().await.satisfy(|events| {
            check!(matches!(events[0], EventType::Init(_)));
            check!(
                events
                    .iter()
                    .filter(|e| matches!(e, EventType::Upgrade(_)))
                    .count()
                    == 1
            );
            check!(
                events
                    .iter()
                    .filter(|e| matches!(e, EventType::MinterPublicKeyFetched { .. }))
                    .count()
                    <= 1
            );
            check!(events[1..].iter().all(|e| matches!(
                e,
                EventType::Upgrade(_) | EventType::MinterPublicKeyFetched { .. }
            )));
        });

        setup.drop().await;
    }

    /// Polls until the minter has fetched its Schnorr master key for the first
    /// time. The fetch is recorded in a `MinterPublicKeyFetched` event, so the
    /// key survives upgrades and the address stays available without waiting.
    async fn wait_for_initial_minter_address(setup: &Setup) {
        for _ in 0..10 {
            if setup
                .minter()
                .get_minter_info()
                .await
                .minter_address
                .is_some()
            {
                return;
            }
            setup.tick().await;
        }
        panic!("Minter address was not cached within timeout");
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
    async fn should_reject_withdrawal_to_reserved_account_key() {
        const SYSTEM_PROGRAM_ID: &str = "11111111111111111111111111111111";
        const WITHDRAWAL_AMOUNT: u64 = 100_000_000;

        let setup = SetupBuilder::new()
            .with_initial_ledger_balances(vec![(
                DEFAULT_CALLER_ACCOUNT,
                Nat::from(WITHDRAWAL_AMOUNT),
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
        let balance_before_withdrawal = setup.ledger().balance_of(DEFAULT_CALLER_ACCOUNT).await;

        let result = setup
            .minter()
            .withdraw(WithdrawalArgs {
                from_subaccount: None,
                amount: Setup::DEFAULT_MINIMUM_WITHDRAWAL_AMOUNT,
                address: SYSTEM_PROGRAM_ID.to_string(),
            })
            .await;

        assert_matches!(result, Err(WithdrawalError::InvalidDestination(_)));
        assert_eq!(
            setup.ledger().balance_of(DEFAULT_CALLER_ACCOUNT).await,
            balance_before_withdrawal
        );

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

        // Deposit and credit a sweep so the minter has enough balance for withdrawals
        deposit_and_credit_funds(&setup).await;

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

        // The withdrawal timer reads the durable nonce account and submits the
        // transaction with the nonce value in place of a recent blockhash.
        setup.advance_time(WITHDRAWAL_PROCESSING_DELAY).await;
        setup
            .execute_http_mocks(
                MockBuilder::with_start_id(32)
                    .submit_withdrawal_transaction()
                    .build(),
            )
            .await;

        let nonce_account = Setup::DEFAULT_NONCE_ACCOUNT.to_string();
        setup.minter().assert_that_events().await.satisfy(|events| {
            check!(events.iter().any(|e| matches!(
                e,
                EventType::CreatedWithdrawalTransaction {
                    burn_indices,
                    nonce_account: account,
                    ..
                } if burn_indices == &[block_index] && account.to_string() == nonce_account
            )));
            check!(events.iter().any(|e| matches!(
                e,
                EventType::SubmittedTransaction {
                    purpose: TransactionPurpose::Withdrawal { burn_indices },
                    ..
                } if burn_indices == &[block_index]
            )));
        });

        // Withdrawal status should be TxSent with some signature
        let status = setup.minter().withdrawal_status(block_index).await;
        let transaction_id = match &status {
            WithdrawalStatus::TxSent { transaction_id } => transaction_id.clone(),
            other => panic!("Expected TxSent, got: {other:?}"),
        };

        // An upgrade replays the created and submitted withdrawal events; the
        // in-flight transaction must survive the replay unchanged.
        setup
            .minter()
            .upgrade(UpgradeArgs::default())
            .await
            .expect("upgrade should succeed");
        assert_eq!(
            setup.minter().withdrawal_status(block_index).await,
            WithdrawalStatus::TxSent {
                transaction_id: transaction_id.clone()
            }
        );

        // Only the in-flight withdrawal is monitored, so the finalization timer
        // reads its nonce account and fetches the landed transaction without
        // fetching a current block.
        setup.advance_time(FINALIZE_TRANSACTIONS_DELAY).await;
        let withdrawal = setup.minter().signed_transaction(&transaction_id).await;
        setup
            .execute_http_mocks(
                MockBuilder::with_start_id(40)
                    .finalize_withdrawal_transaction(&withdrawal)
                    .build(),
            )
            .await;

        let status = setup.minter().withdrawal_status(block_index).await;
        match &status {
            WithdrawalStatus::TxFinalized(TxFinalizedStatus::Success {
                transaction_id: finalized_transaction_id,
                ..
            }) => {
                assert_eq!(*finalized_transaction_id, transaction_id);
            }
            other => panic!("Expected TxFinalized(Success), got: {other:?}"),
        }

        setup.drop().await;
    }
}

mod deposit_sol_tests {
    use super::*;

    const BALANCE_ABOVE_MINIMUM: Lamport = Setup::DEFAULT_MINIMUM_DEPOSIT_AMOUNT + 1;

    #[tokio::test]
    async fn should_queue_deposit_for_caller_if_owner_is_omitted() {
        let setup = SetupBuilder::new().with_proxy_canister().build().await;
        let subaccount = Some([1; 32]);
        let minter = setup.minter().with_http_mocks(
            MockBuilder::new()
                .get_balance(BALANCE_ABOVE_MINIMUM)
                .build(),
        );

        let deposit_id = minter
            .deposit_sol(DepositSolArgs {
                owner: None,
                subaccount,
            })
            .await
            .expect("deposit_sol should queue a sweep");

        assert_eq!(deposit_id, 0);
        assert_eq!(
            minter.deposit_status(deposit_id).await,
            DepositSolStatus::Queued {
                sweepable_amount: BALANCE_ABOVE_MINIMUM - RENT_EXEMPTION_THRESHOLD
            }
        );
        let proxy = setup.proxy_canister_id();
        let deposit_address = setup
            .minter()
            .get_deposit_address(GetDepositAddressArgs {
                owner: None,
                subaccount,
            })
            .await;
        setup.minter().assert_that_events().await.satisfy(|events| {
            let deposit_events: Vec<&EventType> = events[1..]
                .iter()
                .filter(|e| !matches!(e, EventType::MinterPublicKeyFetched { .. }))
                .collect();
            check!(
                deposit_events
                    == [&EventType::QueuedDeposit {
                        deposit_id,
                        account: Account {
                            owner: proxy,
                            subaccount,
                        },
                        address: deposit_address.clone(),
                        balance: BALANCE_ABOVE_MINIMUM,
                    }]
            );
        });

        setup.drop().await;
    }

    #[tokio::test]
    async fn should_charge_balance_read_and_deposit_sol_fee() {
        let setup = SetupBuilder::new().with_proxy_canister().build().await;
        let get_balance_cycles_cost = get_balance_cycles_cost(&setup).await;
        assert!(get_balance_cycles_cost > 0);
        let caller_cycles_before = setup.proxy().cycle_balance().await;
        let minter_cycles_before = setup.minter().cycle_balance().await;

        let result = setup
            .minter()
            .with_http_mocks(
                MockBuilder::new()
                    .get_balance(BALANCE_ABOVE_MINIMUM)
                    .build(),
            )
            .deposit_sol(DEFAULT_CALLER_ACCOUNT)
            .await;

        assert_eq!(result, Ok(0));
        let caller_cycles_after = setup.proxy().cycle_balance().await;
        let minter_cycles_after = setup.minter().cycle_balance().await;
        assert_eq!(
            caller_cycles_before - caller_cycles_after,
            get_balance_cycles_cost + Setup::DEFAULT_DEPOSIT_SOL_FEE
        );
        assert_eq!(
            minter_cycles_after - minter_cycles_before,
            Setup::DEFAULT_DEPOSIT_SOL_FEE
        );

        setup.drop().await;
    }

    #[tokio::test]
    async fn should_treat_default_subaccount_spellings_as_one_account() {
        let setup = SetupBuilder::new().with_proxy_canister().build().await;
        let explicit_default_subaccount = Account {
            subaccount: Some([0; 32]),
            ..DEFAULT_CALLER_ACCOUNT
        };
        let deposit_id = setup
            .minter()
            .with_http_mocks(
                MockBuilder::new()
                    .get_balance(BALANCE_ABOVE_MINIMUM)
                    .build(),
            )
            .deposit_sol(DEFAULT_CALLER_ACCOUNT)
            .await
            .expect("first deposit should be queued");

        let result = setup
            .minter()
            .deposit_sol(explicit_default_subaccount)
            .await;

        assert_eq!(result, Ok(deposit_id));
        let deposit_address = setup
            .minter()
            .get_deposit_address(DEFAULT_CALLER_ACCOUNT)
            .await;
        setup.minter().assert_that_events().await.satisfy(|events| {
            let deposit_events: Vec<&EventType> = events[1..]
                .iter()
                .filter(|e| !matches!(e, EventType::MinterPublicKeyFetched { .. }))
                .collect();
            check!(
                deposit_events
                    == [&EventType::QueuedDeposit {
                        deposit_id,
                        account: DEFAULT_CALLER_ACCOUNT,
                        address: deposit_address.clone(),
                        balance: BALANCE_ABOVE_MINIMUM,
                    }]
            );
        });

        setup.drop().await;
    }

    #[tokio::test]
    async fn should_sweep_finalize_and_mint_queued_deposit_after_timers() {
        const SWEEPABLE_AMOUNT: Lamport = BALANCE_ABOVE_MINIMUM - RENT_EXEMPTION_THRESHOLD;
        let setup = SetupBuilder::new().with_proxy_canister().build().await;
        let deposit_id = setup
            .minter()
            .with_http_mocks(
                MockBuilder::new()
                    .get_balance(BALANCE_ABOVE_MINIMUM)
                    .build(),
            )
            .deposit_sol(DEFAULT_CALLER_ACCOUNT)
            .await
            .expect("deposit_sol should queue a sweep");

        setup.advance_time(SWEEP_DEPOSITS_DELAY).await;
        setup
            .execute_http_mocks(
                MockBuilder::with_start_id(4)
                    .submit_transaction(SUBMISSION_BLOCK_HEIGHT)
                    .build(),
            )
            .await;

        let sweep_signature = assert_matches!(
            setup.minter().deposit_status(deposit_id).await,
            DepositSolStatus::Swept { signature } => signature
        );
        setup.minter().assert_that_events().await.satisfy(|events| {
            check!(events.iter().any(|e| matches!(
                e,
                EventType::SubmittedTransaction {
                    signature,
                    purpose: TransactionPurpose::SweepDeposit { deposit_ids, .. },
                    ..
                } if *signature == sweep_signature && deposit_ids == &[deposit_id]
            )));
        });

        setup.advance_time(FINALIZE_TRANSACTIONS_DELAY).await;
        let sweep = setup.minter().signed_transaction(&sweep_signature).await;
        setup
            .execute_http_mocks(
                MockBuilder::with_start_id(16)
                    .finalize_transaction(&sweep_signature, SUBMISSION_BLOCK_HEIGHT)
                    .get_sweep_transaction(&sweep, SWEEPABLE_AMOUNT)
                    .build(),
            )
            .await;

        setup.wait_for_deposit_minted(deposit_id).await;

        let minted_amount = SWEEPABLE_AMOUNT - FEE_PER_SIGNATURE;
        assert_eq!(
            setup.minter().deposit_status(deposit_id).await,
            DepositSolStatus::Minted {
                block_index: 0,
                minted_amount,
            }
        );
        assert_eq!(
            setup.ledger().balance_of(DEFAULT_CALLER_ACCOUNT).await,
            minted_amount
        );

        setup.drop().await;
    }

    #[tokio::test]
    async fn should_drop_deposit_and_release_account_after_sweep_expires() {
        let setup = SetupBuilder::new().with_proxy_canister().build().await;
        let deposit_id = setup
            .minter()
            .with_http_mocks(
                MockBuilder::new()
                    .get_balance(BALANCE_ABOVE_MINIMUM)
                    .build(),
            )
            .deposit_sol(DEFAULT_CALLER_ACCOUNT)
            .await
            .expect("deposit_sol should queue a sweep");
        setup.advance_time(SWEEP_DEPOSITS_DELAY).await;
        setup
            .execute_http_mocks(
                MockBuilder::with_start_id(4)
                    .submit_transaction(SUBMISSION_BLOCK_HEIGHT)
                    .build(),
            )
            .await;
        let sweep_signature = assert_matches!(
            setup.minter().deposit_status(deposit_id).await,
            DepositSolStatus::Swept { signature } => signature
        );

        setup.advance_time(FINALIZE_TRANSACTIONS_DELAY).await;
        setup
            .execute_http_mocks(
                MockBuilder::with_start_id(16)
                    .mark_transaction_expired(&sweep_signature, EXPIRY_BLOCK_HEIGHT)
                    .build(),
            )
            .await;

        assert_eq!(
            setup.minter().deposit_status(deposit_id).await,
            DepositSolStatus::Dropped {
                signature: sweep_signature.clone()
            }
        );
        setup.minter().assert_that_events().await.satisfy(|events| {
            check!(events.iter().any(|e| matches!(
                e,
                EventType::ExpiredTransaction { signature } if *signature == sweep_signature
            )));
        });
        let setup = setup
            .check_metrics()
            .await
            .assert_contains_metric_matching(r"dropped_deposits 1 \d+")
            .into();

        let deposit_id_after_drop = setup
            .minter()
            .with_http_mocks(
                MockBuilder::with_start_id(28)
                    .get_balance(BALANCE_ABOVE_MINIMUM)
                    .build(),
            )
            .deposit_sol(DEFAULT_CALLER_ACCOUNT)
            .await
            .expect("a dropped deposit releases the account for a new sweep");
        assert_eq!(deposit_id_after_drop, deposit_id + 1);

        setup.drop().await;
    }

    #[tokio::test]
    async fn should_quarantine_deposit_when_finalized_sweep_does_not_match_its_plan() {
        let setup = SetupBuilder::new().with_proxy_canister().build().await;
        let deposit_id = setup
            .minter()
            .with_http_mocks(
                MockBuilder::new()
                    .get_balance(BALANCE_ABOVE_MINIMUM)
                    .build(),
            )
            .deposit_sol(DEFAULT_CALLER_ACCOUNT)
            .await
            .expect("deposit_sol should queue a sweep");
        setup.advance_time(SWEEP_DEPOSITS_DELAY).await;
        setup
            .execute_http_mocks(
                MockBuilder::with_start_id(4)
                    .submit_transaction(SUBMISSION_BLOCK_HEIGHT)
                    .build(),
            )
            .await;
        let sweep_signature = assert_matches!(
            setup.minter().deposit_status(deposit_id).await,
            DepositSolStatus::Swept { signature } => signature
        );

        setup.advance_time(FINALIZE_TRANSACTIONS_DELAY).await;
        let amount_not_in_the_plan = BALANCE_ABOVE_MINIMUM - RENT_EXEMPTION_THRESHOLD + 1;
        let sweep = setup.minter().signed_transaction(&sweep_signature).await;
        setup
            .execute_http_mocks(
                MockBuilder::with_start_id(16)
                    .finalize_transaction(&sweep_signature, SUBMISSION_BLOCK_HEIGHT)
                    .get_sweep_transaction(&sweep, amount_not_in_the_plan)
                    .build(),
            )
            .await;

        assert_eq!(
            setup.minter().deposit_status(deposit_id).await,
            DepositSolStatus::Quarantined {
                signature: sweep_signature.clone()
            }
        );
        setup.minter().assert_that_events().await.satisfy(|events| {
            check!(events.iter().any(|e| matches!(
                e,
                EventType::QuarantinedSweep { signature } if *signature == sweep_signature
            )));
            check!(
                !events
                    .iter()
                    .any(|e| matches!(e, EventType::CreditedSweep { .. }))
            );
        });
        let setup = setup
            .check_metrics()
            .await
            .assert_contains_metric_matching(
                r#"quarantined_swept_deposits\{cause="sweep_unreadable"\} 1 \d+"#,
            )
            .into();

        let result = setup.minter().deposit_sol(DEFAULT_CALLER_ACCOUNT).await;

        assert_eq!(result, Err(DepositSolError::Quarantined { deposit_id }));

        setup.drop().await;
    }

    #[tokio::test]
    async fn should_fail_for_concurrent_access() {
        let setup = SetupBuilder::new().with_proxy_canister().build().await;
        let mocks = SharedMockHttpOutcalls::new(
            MockBuilder::new()
                .get_balance(BALANCE_ABOVE_MINIMUM)
                .build(),
        );
        let minter1 = setup.minter().with_http_mocks(mocks.clone());
        let minter2 = setup.minter().with_http_mocks(mocks.clone());

        let results = join!(
            minter1.deposit_sol(DEFAULT_CALLER_ACCOUNT),
            minter2.deposit_sol(DEFAULT_CALLER_ACCOUNT)
        );

        assert!(
            matches!(
                results,
                (Ok(0), Err(DepositSolError::AlreadyProcessing))
                    | (Err(DepositSolError::AlreadyProcessing), Ok(0))
            ),
            "Expected one queued deposit and one AlreadyProcessing error, got: {results:?}"
        );

        setup.drop().await;
    }

    async fn get_balance_cycles_cost(setup: &Setup) -> u128 {
        let deposit_address: solana_address::Address =
            DEFAULT_CALLER_DEPOSIT_ADDRESS.parse().unwrap();
        setup
            .sol_rpc()
            .get_balance(deposit_address)
            .with_rpc_config(RpcConfig {
                response_size_estimate: None,
                response_consensus: Some(ConsensusStrategy::Threshold {
                    min: 3,
                    total: Some(4),
                }),
            })
            .with_commitment(CommitmentLevel::Finalized)
            .request_cost()
            .send()
            .await
            .expect("Failed to get cycles cost for `getBalance` request")
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
            assert_matches!(result, Err(s) if s.contains("the owner must be non-anonymous"));

            let result = minter
                .try_deposit_sol(DepositSolArgs {
                    owner,
                    subaccount: None,
                })
                .await;
            assert_matches!(result, Err(s) if s.contains("the owner must be non-anonymous"));
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
        assert_matches!(result, Err(s) if s.contains("the owner must be non-anonymous"));

        setup.drop().await;
    }
}

mod minter_owner_tests {
    use super::*;

    #[tokio::test]
    async fn should_fail_for_minter_owner() {
        let setup = SetupBuilder::new().build().await;
        let owner = Some(setup.minter_canister_id());
        let expected_message = format!(
            "the minter's own principal {} is not a valid deposit owner",
            setup.minter_canister_id()
        );

        let result = setup
            .minter()
            .try_get_deposit_address(GetDepositAddressArgs {
                owner,
                subaccount: None,
            })
            .await;
        assert_matches!(result, Err(s) if s.contains(&expected_message));

        let result = setup
            .minter()
            .try_deposit_sol(DepositSolArgs {
                owner,
                subaccount: None,
            })
            .await;
        assert_matches!(result, Err(s) if s.contains(&expected_message));

        setup.drop().await;
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
            // Only the init and minter public key events should have been recorded
            .assert_contains_metric_matching(r#"total_event_count 2 \d+"#)
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
