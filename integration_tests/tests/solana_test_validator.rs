use candid::Principal;
use cksol_int_tests::{
    Setup,
    fixtures::MINTER_ADDRESS,
    ledger_init_args::LEDGER_TRANSFER_FEE,
    validator::{
        FEE_PER_SIGNATURE, RENT_EXEMPTION_THRESHOLD, SolanaTestValidator,
        wait_for_withdrawal_finalized,
    },
};
use cksol_types::{DepositStatus, ProcessDepositArgs, Signature, WithdrawalArgs, WithdrawalStatus};
use cksol_types_internal::{
    event::{EventType, TransactionPurpose},
    log::Priority,
};
use icrc_ledger_types::icrc1::account::Account;
use itertools::Itertools;
use sol_rpc_types::Lamport;
use solana_address::Address;
use solana_keypair::{Keypair, Signer};
use solana_native_token::LAMPORTS_PER_SOL;
use std::time::Duration;

const DEPOSITOR: Principal = Setup::DEFAULT_CALLER;

// TODO DEFI-2643: Add tests with more exotic transactions, e.g.:
//  - a transaction with multiple transfer instructions to same target address: single mint with the summed up amount
//  - a transaction with multiple instructions, not all to the same target address: only relevant amounts are considered.

#[tokio::test(flavor = "multi_thread")]
async fn should_deposit_consolidate_and_withdraw() {
    let validator = SolanaTestValidator::start().await;
    let setup = validator.setup().await;

    let withdrawal_destination = Keypair::new();
    let withdrawal_address = withdrawal_destination.pubkey();

    for (i, num_deposits) in [1_u8, 15].into_iter().enumerate() {
        println!("Testing with {num_deposits} deposit(s)");

        let minter_cycles_before = setup.minter().cycle_balance().await;
        let minter_sol_before = validator.get_balance(&MINTER_ADDRESS).await;
        let destination_sol_before = validator.get_balance(&withdrawal_address).await;

        let accounts: Vec<_> = (1_u8..=num_deposits)
            .map(|j| Account {
                owner: DEPOSITOR,
                // Make sure the accounts are unique across all iterations
                subaccount: Some([i as u8 + j; 32]),
            })
            .collect();

        // Deposit funds
        let (deposit_addresses, deposit_amounts, minted_amounts): (Vec<_>, Vec<_>, Vec<_>) =
            futures::future::join_all(accounts.iter().enumerate().map(async |(j, account)| {
                let deposit_amount = ((j as u64 + 1) * LAMPORTS_PER_SOL) / 10;
                let (deposit_address, minted_amount) = validator
                    .deposit_to_account(&setup, *account, deposit_amount)
                    .await;
                (deposit_address, deposit_amount, minted_amount)
            }))
            .await
            .into_iter()
            .multiunzip();

        let total_minted_amount = minted_amounts.iter().sum::<Lamport>();
        let total_deposited_amount = deposit_amounts.iter().sum::<Lamport>();

        let deposit_accounts_balances_before = validator.get_balances(&deposit_addresses).await;

        // Trigger consolidation and wait for the minter's Solana balance to increase
        setup.advance_time(Duration::from_mins(10)).await;
        validator
            .wait_for_finalized_balance(&MINTER_ADDRESS, minter_sol_before)
            .await;

        // Verify deposit addresses were drained
        for (deposit_address, &balance_before, &deposit_amount) in itertools::multizip((
            &deposit_addresses,
            &deposit_accounts_balances_before,
            &deposit_amounts,
        )) {
            let balance_after = validator.get_balance(deposit_address).await;
            assert_eq!(balance_after, balance_before - deposit_amount);
        }

        let minter_sol_after_consolidation = validator.get_balance(&MINTER_ADDRESS).await;
        assert_eq!(
            minter_sol_after_consolidation,
            // Each deposit address is a signer in its consolidation transaction, so
            // the total Solana transaction fee is `FEE_PER_SIGNATURE` per deposit.
            minter_sol_before + total_deposited_amount - num_deposits as u64 * FEE_PER_SIGNATURE
        );

        let minter_cycles_after = setup.minter().cycle_balance().await;
        assert!(
            minter_cycles_after >= minter_cycles_before,
            "Minter cycles balance decreased"
        );

        // Withdraw the full minted amount from each depositor account (in parallel)
        let burn_indices: Vec<_> =
            futures::future::join_all(accounts.iter().zip(&minted_amounts).map(
                async |(account, &minted_amount)| {
                    // Approve charges LEDGER_TRANSFER_FEE, so we can only withdraw the remainder
                    let withdrawal_amount = minted_amount - LEDGER_TRANSFER_FEE;

                    setup
                        .ledger()
                        .approve(
                            account.subaccount,
                            withdrawal_amount,
                            setup.minter_account(),
                        )
                        .await;

                    setup
                        .minter()
                        .withdraw(WithdrawalArgs {
                            from_subaccount: account.subaccount,
                            amount: withdrawal_amount,
                            address: withdrawal_address.to_string(),
                        })
                        .await
                        .expect("withdraw should succeed")
                        .block_index
                },
            ))
            .await;

        // Advance time to trigger withdrawal processing and monitor timers
        setup.advance_time(Duration::from_mins(10)).await;

        for &burn_index in &burn_indices {
            wait_for_withdrawal_finalized(&setup, burn_index).await;
        }

        // Verify all ICRC accounts are drained
        for account in &accounts {
            let balance = setup.ledger().balance_of(*account).await;
            assert_eq!(
                balance, 0,
                "Account {account:?} should have zero ckSOL balance"
            );
        }

        // Verify the destination received the expected SOL for this iteration
        let per_withdrawal_fees = LEDGER_TRANSFER_FEE + Setup::DEFAULT_WITHDRAWAL_FEE;
        let expected_received = total_minted_amount - num_deposits as u64 * per_withdrawal_fees;
        let destination_sol_after = validator.get_balance(&withdrawal_address).await;
        assert_eq!(
            destination_sol_after - destination_sol_before,
            expected_received
        );

        // Minter should retain at least its initial SOL balance (withdrawal fees stay with it)
        let minter_sol_final = validator.get_balance(&MINTER_ADDRESS).await;
        assert!(
            minter_sol_final >= minter_sol_before,
            "Minter SOL balance should not decrease"
        );
    }

    setup.drop().await;
}

const DEPOSIT_CONSOLIDATION_DELAY: Duration = Duration::from_mins(10);
const RESUBMIT_TRANSACTIONS_DELAY: Duration = Duration::from_mins(3);
const WITHDRAWAL_PROCESSING_DELAY: Duration = Duration::from_mins(1);
const SUB_RENT_REMAINDER: Lamport = 500_000;
const WITHDRAWAL_ROUNDS_WITHOUT_PROGRESS: usize = 3;
const _: () = assert!(SUB_RENT_REMAINDER < RENT_EXEMPTION_THRESHOLD);

#[tokio::test(flavor = "multi_thread")]
async fn should_strand_deposit_when_address_keeps_sub_rent_remainder() {
    let validator = SolanaTestValidator::start().await;
    let setup = validator.setup().await;
    let account = Account {
        owner: DEPOSITOR,
        subaccount: Some([42; 32]),
    };
    let deposit_address: Address = setup.minter().get_deposit_address(account).await.into();

    let first_deposit = validator
        .transfer_to(deposit_address, Setup::DEFAULT_MINIMUM_DEPOSIT_AMOUNT)
        .await;
    validator
        .transfer_to(deposit_address, SUB_RENT_REMAINDER)
        .await;
    let deposit_address_balance = Setup::DEFAULT_MINIMUM_DEPOSIT_AMOUNT + SUB_RENT_REMAINDER;
    assert_eq!(
        validator.get_balance(&deposit_address).await,
        deposit_address_balance
    );

    let (mint_block_index, minted_amount) = match setup
        .minter()
        .process_deposit(ProcessDepositArgs {
            owner: Some(account.owner),
            subaccount: account.subaccount,
            signature: first_deposit.into(),
        })
        .await
    {
        Ok(DepositStatus::Minted {
            block_index,
            minted_amount,
            ..
        }) => (block_index, minted_amount),
        other => panic!("Expected the first deposit to be minted, got: {other:?}"),
    };
    assert_eq!(
        minted_amount,
        Setup::DEFAULT_MINIMUM_DEPOSIT_AMOUNT - Setup::DEFAULT_MANUAL_DEPOSIT_FEE
    );

    let minter_sol_before = validator.get_balance(&MINTER_ADDRESS).await;
    let minter_cycles_before = setup.minter().cycle_balance().await;

    setup
        .advance_time_and_settle(DEPOSIT_CONSOLIDATION_DELAY)
        .await;
    let consolidation_signature = wait_for_consolidation_submitted(&setup, mint_block_index).await;

    let rejections = wait_for_log_entries(&setup, INSUFFICIENT_FUNDS_FOR_RENT, 1).await;
    println!("Consolidation rejected by preflight: {}", rejections[0]);
    assert!(
        !validator
            .has_seen_transaction(&consolidation_signature.into())
            .await,
        "The rejected consolidation must never reach the chain"
    );

    let resubmissions = wait_for_resubmissions(&setup, 2).await;
    for resubmitted_signature in resubmissions {
        assert!(
            !validator
                .has_seen_transaction(&resubmitted_signature.into())
                .await,
            "A resubmitted consolidation must never reach the chain either"
        );
    }
    wait_for_log_entries(&setup, INSUFFICIENT_FUNDS_FOR_RENT, 3).await;

    assert_eq!(
        validator.get_balance(&deposit_address).await,
        deposit_address_balance,
        "The deposit stays stranded on the deposit address"
    );
    assert_eq!(
        validator.get_balance(&MINTER_ADDRESS).await,
        minter_sol_before,
        "Nothing reaches the minter's main address"
    );
    assert_eq!(setup.minter().get_minter_info().await.balance, 0);
    assert!(
        setup.minter().cycle_balance().await < minter_cycles_before,
        "Every resubmission burns cycles for a new threshold signature"
    );

    let withdrawal_amount = minted_amount - LEDGER_TRANSFER_FEE;
    setup
        .ledger()
        .approve(
            account.subaccount,
            withdrawal_amount,
            setup.minter_account(),
        )
        .await;
    let burn_index = setup
        .minter()
        .withdraw(WithdrawalArgs {
            from_subaccount: account.subaccount,
            amount: withdrawal_amount,
            address: Keypair::new().pubkey().to_string(),
        })
        .await
        .expect("withdraw should succeed")
        .block_index;
    for _ in 0..WITHDRAWAL_ROUNDS_WITHOUT_PROGRESS {
        setup
            .advance_time_and_settle(WITHDRAWAL_PROCESSING_DELAY)
            .await;
        assert_eq!(
            setup.minter().withdrawal_status(burn_index).await,
            WithdrawalStatus::Pending,
            "The withdrawal starves because the stranded deposit never funds the minter"
        );
    }

    setup.drop().await;
}

const INSUFFICIENT_FUNDS_FOR_RENT: &str = "insufficient funds for rent";

async fn wait_for_consolidation_submitted(setup: &Setup, mint_block_index: u64) -> Signature {
    wait_for_events(setup, "consolidation submission", |events| {
        events.iter().find_map(|event| match event {
            EventType::SubmittedTransaction {
                signature,
                purpose: TransactionPurpose::ConsolidateDeposits { mint_indices },
                ..
            } if mint_indices == &[mint_block_index] => Some(signature.clone()),
            _ => None,
        })
    })
    .await
}

async fn wait_for_resubmissions(setup: &Setup, count: usize) -> Vec<Signature> {
    wait_for_events(setup, &format!("{count} resubmissions"), |events| {
        let resubmissions: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                EventType::ResubmittedTransaction { new_signature, .. } => {
                    Some(new_signature.clone())
                }
                _ => None,
            })
            .collect();
        (resubmissions.len() >= count).then_some(resubmissions)
    })
    .await
}

async fn wait_for_events<T>(
    setup: &Setup,
    description: &str,
    extract: impl Fn(&[EventType]) -> Option<T>,
) -> T {
    poll_minter(setup, description, async || {
        let events: Vec<_> = setup
            .minter()
            .get_all_events()
            .await
            .into_iter()
            .map(|event| event.payload)
            .collect();
        extract(&events)
    })
    .await
}

async fn wait_for_log_entries(setup: &Setup, needle: &str, count: usize) -> Vec<String> {
    let description = format!("{count} log entries mentioning {needle:?}");
    poll_minter(setup, &description, async || {
        let matching: Vec<_> = setup
            .minter()
            .retrieve_logs(&Priority::Info)
            .await
            .into_iter()
            .map(|entry| entry.message)
            .filter(|message| message.contains(needle))
            .collect();
        (matching.len() >= count).then_some(matching)
    })
    .await
}

/// Polls the minter until `probe` finds what it is looking for, advancing time
/// between polls by enough for every minter timer to fire.
async fn poll_minter<T>(setup: &Setup, description: &str, probe: impl AsyncFn() -> Option<T>) -> T {
    const MAX_ATTEMPTS: usize = 20;
    for _ in 0..MAX_ATTEMPTS {
        if let Some(found) = probe().await {
            return found;
        }
        setup
            .advance_time_and_settle(RESUBMIT_TRANSACTIONS_DELAY)
            .await;
    }
    panic!("Timed out waiting for {description}");
}
