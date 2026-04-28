use crate::{
    constants::GET_TRANSACTION_CYCLES,
    deposit::manual::process_deposit,
    state::event::{DepositId, DepositSource, EventType},
    storage::reset_events,
    test_fixtures::{
        BLOCK_INDEX, DEPOSIT_CONSOLIDATION_FEE, EventsAssert, MANUAL_DEPOSIT_FEE,
        PROCESS_DEPOSIT_REQUIRED_CYCLES,
        deposit::{
            DEPOSIT_AMOUNT, DEPOSITOR_ACCOUNT, DEPOSITOR_PRINCIPAL, accepted_deposit_event,
            deposit_status_minted, deposit_status_processing, deposit_status_quarantined,
            deposit_transaction_to_multiple_accounts,
            deposit_transaction_to_multiple_accounts_signature,
            deposit_transaction_to_wrong_address, deposit_transaction_to_wrong_address_signature,
            legacy_deposit_transaction, legacy_deposit_transaction_signature, minted_event,
            quarantined_deposit_event, v0_deposit_transaction, v0_deposit_transaction_signature,
        },
        init_schnorr_master_key, init_state, init_state_with_args,
        runtime::TestCanisterRuntime,
        valid_init_args,
    },
};
use assert_matches::assert_matches;
use candid_parser::Principal;
use cksol_types::{DepositStatus, InsufficientCyclesError, ProcessDepositError};
use cksol_types_internal::InitArgs;
use ic_canister_runtime::IcError;
use icrc_ledger_types::icrc1::{account::Account, transfer::TransferError};
use sol_rpc_types::{EncodedConfirmedTransactionWithStatusMeta, Lamport};

mod process_deposit_tests {
    use super::*;

    #[tokio::test]
    async fn should_fail_if_insufficient_cycles_attached() {
        init_state();

        let runtime = TestCanisterRuntime::new()
            .add_msg_cycles_available(PROCESS_DEPOSIT_REQUIRED_CYCLES - 1);

        let result = process_deposit(
            runtime,
            DEPOSITOR_ACCOUNT,
            legacy_deposit_transaction_signature(),
        )
        .await;

        assert_eq!(
            result,
            Err(ProcessDepositError::InsufficientCycles(
                InsufficientCyclesError {
                    expected: PROCESS_DEPOSIT_REQUIRED_CYCLES,
                    received: PROCESS_DEPOSIT_REQUIRED_CYCLES - 1,
                }
            ))
        );
        EventsAssert::assert_no_events_recorded();
    }

    #[tokio::test]
    async fn should_return_error_if_get_transaction_fails() {
        init_state();
        init_schnorr_master_key();

        let runtime = rejected_runtime().add_stub_error(IcError::CallPerformFailed);

        let result = process_deposit(
            runtime,
            DEPOSITOR_ACCOUNT,
            legacy_deposit_transaction_signature(),
        )
        .await;

        assert_matches!(
            result,
            Err(ProcessDepositError::TemporarilyUnavailable(e)) => assert!(e.contains("Inter-canister call perform failed"))
        );
        EventsAssert::assert_no_events_recorded();
    }

    #[tokio::test]
    async fn should_return_error_if_transaction_not_found() {
        init_state();
        init_schnorr_master_key();

        let runtime = rejected_runtime().add_get_transaction_not_found();

        let result = process_deposit(
            runtime,
            DEPOSITOR_ACCOUNT,
            legacy_deposit_transaction_signature(),
        )
        .await;

        assert_eq!(result, Err(ProcessDepositError::TransactionNotFound));
        EventsAssert::assert_no_events_recorded();
    }

    #[tokio::test]
    async fn should_return_error_if_transaction_not_valid_deposit() {
        init_state();
        init_schnorr_master_key();

        let runtime =
            rejected_runtime().add_get_transaction_response(deposit_transaction_to_wrong_address());

        let result = process_deposit(
            runtime,
            DEPOSITOR_ACCOUNT,
            deposit_transaction_to_wrong_address_signature(),
        )
        .await;

        assert_matches!(
            result,
            Err(ProcessDepositError::InvalidDepositTransaction(e)) => assert!(e.contains("Transaction must target deposit address"))
        );
        EventsAssert::assert_no_events_recorded();
    }

    #[tokio::test]
    async fn should_fail_if_deposit_amount_is_below_minimum() {
        const MINIMUM_DEPOSIT_AMOUNT: Lamport = 2 * DEPOSIT_AMOUNT;
        init_state_with_args(InitArgs {
            minimum_deposit_amount: MINIMUM_DEPOSIT_AMOUNT,
            ..valid_init_args()
        });
        init_schnorr_master_key();

        let runtime = rejected_runtime().add_get_transaction_response(legacy_deposit_transaction());

        let result = process_deposit(
            runtime,
            DEPOSITOR_ACCOUNT,
            legacy_deposit_transaction_signature(),
        )
        .await;

        assert_eq!(
            result,
            Err(ProcessDepositError::ValueTooSmall {
                deposit_amount: DEPOSIT_AMOUNT,
                minimum_deposit_amount: MINIMUM_DEPOSIT_AMOUNT,
            })
        );
        EventsAssert::assert_no_events_recorded();
    }

    #[tokio::test]
    async fn should_return_processing_if_mint_fails() {
        init_state();
        init_schnorr_master_key();

        let runtime = runtime(legacy_deposit_transaction())
            .add_icrc1_transfer_response(Err(TransferError::TemporarilyUnavailable));

        let result = process_deposit(
            runtime,
            DEPOSITOR_ACCOUNT,
            legacy_deposit_transaction_signature(),
        )
        .await;

        assert_eq!(result, Ok(deposit_status_processing()));

        EventsAssert::from_recorded()
            .expect_event_eq(accepted_deposit_event())
            .assert_no_more_events();
    }

    #[tokio::test]
    async fn should_successfully_mint_on_second_call() {
        init_state();
        init_schnorr_master_key();

        // First call: makes JSON-RPC call and attempts to mint
        let runtime = runtime(legacy_deposit_transaction())
            .add_icrc1_transfer_response(Err(TransferError::TemporarilyUnavailable));
        let result = process_deposit(
            runtime,
            DEPOSITOR_ACCOUNT,
            legacy_deposit_transaction_signature(),
        )
        .await;
        assert_eq!(result, Ok(deposit_status_processing()));

        // Second call: fetches status from minter state, and mints successfully without making any
        // additional JSON-RPC calls
        let runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .add_icrc1_transfer_response(Ok(BLOCK_INDEX.into()));
        let result = process_deposit(
            runtime,
            DEPOSITOR_ACCOUNT,
            legacy_deposit_transaction_signature(),
        )
        .await;
        assert_eq!(result, Ok(deposit_status_minted()));

        EventsAssert::from_recorded()
            .expect_event_eq(accepted_deposit_event())
            .expect_event_eq(minted_event(BLOCK_INDEX))
            .assert_no_more_events();
    }

    #[tokio::test]
    async fn should_succeed_with_valid_deposit_transaction() {
        init_state();
        init_schnorr_master_key();

        for (block_index, transaction, signature) in [
            (
                BLOCK_INDEX,
                legacy_deposit_transaction(),
                legacy_deposit_transaction_signature(),
            ),
            (
                BLOCK_INDEX + 1,
                v0_deposit_transaction(),
                v0_deposit_transaction_signature(),
            ),
        ] {
            reset_events();

            let runtime = runtime(transaction).add_icrc1_transfer_response(Ok(block_index.into()));

            let result = process_deposit(runtime, DEPOSITOR_ACCOUNT, signature).await;

            assert_eq!(
                result,
                Ok(DepositStatus::Minted {
                    block_index,
                    minted_amount: DEPOSIT_AMOUNT - MANUAL_DEPOSIT_FEE,
                    deposit_id: cksol_types::DepositId {
                        signature: signature.into(),
                        account: DEPOSITOR_ACCOUNT,
                    },
                })
            );

            EventsAssert::from_recorded()
                .expect_event_eq(EventType::AcceptedDeposit {
                    deposit_id: DepositId {
                        signature,
                        account: DEPOSITOR_ACCOUNT,
                    },
                    deposit_amount: DEPOSIT_AMOUNT,
                    amount_to_mint: DEPOSIT_AMOUNT - MANUAL_DEPOSIT_FEE,
                    source: DepositSource::Manual,
                })
                .expect_event_eq(EventType::Minted {
                    deposit_id: DepositId {
                        signature,
                        account: DEPOSITOR_ACCOUNT,
                    },
                    mint_block_index: block_index.into(),
                })
                .assert_no_more_events();
        }
    }

    #[tokio::test]
    async fn should_not_double_mint() {
        init_state();
        init_schnorr_master_key();

        // Successful mint
        let runtime = runtime(legacy_deposit_transaction())
            .add_icrc1_transfer_response(Ok(BLOCK_INDEX.into()));
        let result = process_deposit(
            runtime,
            DEPOSITOR_ACCOUNT,
            legacy_deposit_transaction_signature(),
        )
        .await;
        assert_eq!(result, Ok(deposit_status_minted()));

        // Second call: returns the same status
        let runtime = TestCanisterRuntime::new();
        let result = process_deposit(
            runtime,
            DEPOSITOR_ACCOUNT,
            legacy_deposit_transaction_signature(),
        )
        .await;
        assert_eq!(result, Ok(deposit_status_minted()));

        // Only one mint event recorded
        EventsAssert::from_recorded()
            .expect_event_eq(accepted_deposit_event())
            .expect_event_eq(minted_event(BLOCK_INDEX))
            .assert_no_more_events();
    }

    #[tokio::test]
    async fn should_quarantine_deposit() {
        init_state();
        init_schnorr_master_key();

        // Don't mock the ledger response so the runtime panics when calling it to mint
        let runtime = runtime(legacy_deposit_transaction());
        let first_result = tokio::spawn(async move {
            process_deposit(
                runtime,
                DEPOSITOR_ACCOUNT,
                legacy_deposit_transaction_signature(),
            )
            .await
        })
        .await;
        assert!(first_result.is_err_and(|e| e.is_panic()));

        // On the second call, the deposit should have been quarantined
        let runtime = TestCanisterRuntime::new();
        let second_result = process_deposit(
            runtime,
            DEPOSITOR_ACCOUNT,
            legacy_deposit_transaction_signature(),
        )
        .await;
        assert_eq!(second_result, Ok(deposit_status_quarantined()));

        // Calling `process_deposit` again for the same deposit should return the same status
        let runtime = TestCanisterRuntime::new();
        let third_result = process_deposit(
            runtime,
            DEPOSITOR_ACCOUNT,
            legacy_deposit_transaction_signature(),
        )
        .await;
        assert_eq!(third_result, second_result);

        // Only one mint event recorded
        EventsAssert::from_recorded()
            .expect_event_eq(accepted_deposit_event())
            .expect_event_eq(quarantined_deposit_event())
            .assert_no_more_events();
    }

    #[tokio::test]
    async fn should_allow_deposits_to_multiple_accounts_with_single_transaction() {
        const ACCOUNTS: [Account; 3] = [
            Account {
                owner: DEPOSITOR_PRINCIPAL,
                subaccount: None,
            },
            Account {
                owner: DEPOSITOR_PRINCIPAL,
                subaccount: Some([1; 32]),
            },
            Account {
                owner: Principal::from_slice(&[0xa; 29]),
                subaccount: Some([2; 32]),
            },
        ];
        const DEPOSIT_AMOUNTS: [Lamport; 3] = [
            100_000_000, // 0.1 SOL
            200_000_000, // 0.2 SOL
            300_000_000, // 0.3 SOL
        ];
        const BLOCK_INDEXES: [u64; 3] = [79853, 79854, 79855];

        init_state();
        init_schnorr_master_key();

        for i in 0..3 {
            let runtime = runtime(deposit_transaction_to_multiple_accounts())
                .add_icrc1_transfer_response(Ok(BLOCK_INDEXES[i].into()));
            let result = process_deposit(
                runtime,
                ACCOUNTS[i],
                deposit_transaction_to_multiple_accounts_signature(),
            )
            .await;
            assert_eq!(
                result,
                Ok(DepositStatus::Minted {
                    block_index: BLOCK_INDEXES[i],
                    minted_amount: DEPOSIT_AMOUNTS[i] - MANUAL_DEPOSIT_FEE,
                    deposit_id: cksol_types::DepositId {
                        signature: deposit_transaction_to_multiple_accounts_signature().into(),
                        account: ACCOUNTS[i],
                    },
                })
            );
        }

        let mut events_assert = EventsAssert::from_recorded();
        for i in 0..3 {
            let deposit_id = DepositId {
                signature: deposit_transaction_to_multiple_accounts_signature(),
                account: ACCOUNTS[i],
            };
            events_assert = events_assert
                .expect_event_eq(EventType::AcceptedDeposit {
                    deposit_id,
                    deposit_amount: DEPOSIT_AMOUNTS[i],
                    amount_to_mint: DEPOSIT_AMOUNTS[i] - MANUAL_DEPOSIT_FEE,
                    source: DepositSource::Manual,
                })
                .expect_event_eq(EventType::Minted {
                    deposit_id,
                    mint_block_index: BLOCK_INDEXES[i].into(),
                })
        }
        events_assert.assert_no_more_events();
    }

    /// Half the `getTransaction` RPC budget; the other half is refunded by the RPC provider.
    const RPC_COST: u128 = GET_TRANSACTION_CYCLES / 2;

    /// Runtime for a `process_deposit` call that accepts the given transaction.
    /// Charges the RPC cost + consolidation fee and bakes in the transaction as the `getTransaction` stub.
    fn runtime(
        get_transaction_result: impl TryInto<EncodedConfirmedTransactionWithStatusMeta>,
    ) -> TestCanisterRuntime {
        base_runtime(DEPOSIT_CONSOLIDATION_FEE).add_get_transaction_response(get_transaction_result)
    }

    /// Runtime for a `process_deposit` call that does not accept a deposit.
    /// Charges only the RPC cost; caller chains the stub response or error.
    fn rejected_runtime() -> TestCanisterRuntime {
        base_runtime(0)
    }

    /// Shared cycles setup used by both `runtime` and `rejected_runtime`.
    fn base_runtime(consolidation_fee: u128) -> TestCanisterRuntime {
        TestCanisterRuntime::new()
            .with_increasing_time()
            .add_msg_cycles_available(PROCESS_DEPOSIT_REQUIRED_CYCLES)
            .add_msg_cycles_refunded(GET_TRANSACTION_CYCLES - RPC_COST)
            .add_msg_cycles_accept(RPC_COST + consolidation_fee)
    }
}
