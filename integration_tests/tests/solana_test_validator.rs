use candid::Principal;
use cksol_int_tests::{
    Setup,
    fixtures::MINTER_ADDRESS,
    ledger_init_args::LEDGER_TRANSFER_FEE,
    validator::{FEE_PER_SIGNATURE, SolanaTestValidator, wait_for_withdrawal_finalized},
};
use cksol_types::WithdrawalArgs;
use icrc_ledger_types::icrc1::account::Account;
use itertools::Itertools;
use sol_rpc_types::Lamport;
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
