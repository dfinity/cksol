use candid::Principal;
use cksol_int_tests::{
    Setup,
    fixtures::{MINTER_ADDRESS, RENT_EXEMPTION_THRESHOLD},
    ledger_init_args::LEDGER_TRANSFER_FEE,
    validator::{FEE_PER_SIGNATURE, SolanaTestValidator, wait_for_withdrawal_finalized},
};
use cksol_types::{DepositSolId, DepositSolStatus, Signature, WithdrawalArgs};
use cksol_types_internal::UpgradeArgs;
use icrc_ledger_types::icrc1::account::Account;
use sol_rpc_types::Lamport;
use solana_address::Address;
use solana_keypair::{Keypair, Signer};
use solana_native_token::LAMPORTS_PER_SOL;
use std::time::Duration;

const DEPOSITOR: Principal = Setup::DEFAULT_CALLER;

#[tokio::test(flavor = "multi_thread")]
async fn should_deposit_and_withdraw() {
    let validator = SolanaTestValidator::start().await;
    let setup = validator.setup().await;

    let withdrawal_destination = Keypair::new();
    let withdrawal_address = withdrawal_destination.pubkey();

    for (i, num_deposits) in [1_u8, 15].into_iter().enumerate() {
        println!("Testing with {num_deposits} deposit(s)");

        let minter_cycles_before = setup.minter().cycle_balance().await;
        let minter_sol_before = validator.get_balance(&MINTER_ADDRESS).await;
        let minter_info_balance_before = setup.minter().get_minter_info().await.balance;
        let destination_sol_before = validator.get_balance(&withdrawal_address).await;

        let accounts: Vec<_> = (1_u8..=num_deposits)
            .map(|j| Account {
                owner: DEPOSITOR,
                // Make sure the accounts are unique across all iterations
                subaccount: Some([i as u8 + j; 32]),
            })
            .collect();

        let (deposit_addresses, deposit_amounts): (Vec<_>, Vec<_>) =
            futures::future::join_all(accounts.iter().enumerate().map(async |(j, account)| {
                let deposit_amount = ((j as u64 + 1) * LAMPORTS_PER_SOL) / 10;
                let deposit_address = validator
                    .fund_deposit_address(&setup, *account, deposit_amount)
                    .await;
                (deposit_address, deposit_amount)
            }))
            .await
            .into_iter()
            .unzip::<_, _, Vec<Address>, Vec<Lamport>>();

        let deposit_ids: Vec<DepositSolId> =
            futures::future::join_all(accounts.iter().map(async |&account| {
                setup
                    .minter()
                    .deposit_sol(account)
                    .await
                    .expect("deposit_sol should queue a sweep")
            }))
            .await;

        // Every deposit address signs the sweep of its sweepable amount, so the
        // total Solana transaction fee is `FEE_PER_SIGNATURE` per deposit.
        let total_sweepable_amount: Lamport = deposit_amounts
            .iter()
            .map(|deposit_amount| deposit_amount - RENT_EXEMPTION_THRESHOLD)
            .sum();
        let total_sweep_fee = num_deposits as u64 * FEE_PER_SIGNATURE;
        let expected_minter_sol_after_sweep =
            minter_sol_before + total_sweepable_amount - total_sweep_fee;

        // Trigger the sweep and wait for the minter to hold the swept deposits
        setup.advance_time(Duration::from_mins(1)).await;
        validator
            .wait_for_finalized_balance(&MINTER_ADDRESS, expected_minter_sol_after_sweep)
            .await;

        // Verify the deposit addresses were drained down to the rent exemption threshold
        for deposit_address in &deposit_addresses {
            let balance_after = validator.get_balance(deposit_address).await;
            assert_eq!(balance_after, RENT_EXEMPTION_THRESHOLD);
        }

        // Wait for the mints and verify the minted amounts and ledger balances
        let mut minted_amounts = Vec::new();
        for ((&deposit_id, account), &deposit_amount) in
            deposit_ids.iter().zip(&accounts).zip(&deposit_amounts)
        {
            let expected_minted_amount =
                deposit_amount - RENT_EXEMPTION_THRESHOLD - FEE_PER_SIGNATURE;
            let minted_amount = setup.wait_for_deposit_minted(deposit_id).await;
            assert_eq!(minted_amount, expected_minted_amount);
            assert_eq!(
                setup.ledger().balance_of(*account).await,
                expected_minted_amount
            );
            minted_amounts.push(minted_amount);
        }
        let total_minted_amount = minted_amounts.iter().sum::<Lamport>();

        assert_eq!(
            setup.minter().get_minter_info().await.balance,
            minter_info_balance_before + total_sweepable_amount - total_sweep_fee
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

#[tokio::test(flavor = "multi_thread")]
async fn should_add_an_operator_created_nonce_account_through_an_upgrade() {
    let validator = SolanaTestValidator::start().await;
    let setup = validator
        .setup_builder()
        .with_nonce_accounts(Vec::new())
        .build()
        .await;

    assert_eq!(
        setup.minter().get_minter_info().await.nonce_accounts,
        Vec::<String>::new()
    );
    let authority = setup.wait_for_minter_address().await;

    let nonce_accounts: Vec<String> = validator
        .create_nonce_accounts(1, &authority)
        .await
        .iter()
        .map(Address::to_string)
        .collect();

    setup
        .minter()
        .upgrade(UpgradeArgs {
            nonce_accounts_to_add: Some(nonce_accounts.clone()),
            ..UpgradeArgs::default()
        })
        .await
        .expect("upgrade should succeed");

    assert_eq!(
        setup.minter().get_minter_info().await.nonce_accounts,
        nonce_accounts
    );

    setup.drop().await;
}

async fn wait_for_minter_balance(setup: &Setup, expected_balance: Lamport) {
    for _ in 0..30 {
        if setup.minter().get_minter_info().await.balance == expected_balance {
            return;
        }
        setup.advance_time_and_settle(Duration::from_mins(1)).await;
    }
    panic!("Minter balance did not reach {expected_balance} within timeout");
}

/// The largest number of deposits the minter sweeps in a single Solana transaction.
const MAX_DEPOSITS_PER_SWEEP: usize = 10;

#[tokio::test(flavor = "multi_thread")]
async fn should_sweep_a_full_batch_of_deposits_in_one_transaction() {
    let validator = SolanaTestValidator::start().await;
    let setup = validator.setup().await;

    let accounts: Vec<Account> = (1..=MAX_DEPOSITS_PER_SWEEP as u8)
        .map(|i| Account {
            owner: Principal::from_slice(&[i; 10]),
            subaccount: Some([i; 32]),
        })
        .collect();
    let deposit_amounts: Vec<Lamport> = (1..=MAX_DEPOSITS_PER_SWEEP as Lamport)
        .map(|i| (i + 2) * LAMPORTS_PER_SOL / 100)
        .collect();

    let deposit_addresses: Vec<Address> =
        futures::future::join_all(accounts.iter().zip(&deposit_amounts).map(
            async |(&account, &deposit_amount)| {
                validator
                    .fund_deposit_address(&setup, account, deposit_amount)
                    .await
            },
        ))
        .await;
    let minter_sol_before = validator.get_balance(&MINTER_ADDRESS).await;

    let deposit_ids: Vec<DepositSolId> =
        futures::future::join_all(accounts.iter().map(async |&account| {
            setup
                .minter()
                .deposit_sol(account)
                .await
                .expect("deposit_sol should queue a sweep")
        }))
        .await;

    for (&deposit_id, &deposit_amount) in deposit_ids.iter().zip(&deposit_amounts) {
        assert_eq!(
            setup.minter().deposit_status(deposit_id).await,
            DepositSolStatus::Queued {
                sweepable_amount: deposit_amount - RENT_EXEMPTION_THRESHOLD
            }
        );
    }

    // Every deposit address signs the sweep, so the transaction costs one signature fee
    // per deposit and the largest deposit pays all of them.
    let total_sweepable_amount: Lamport = deposit_amounts
        .iter()
        .map(|deposit_amount| deposit_amount - RENT_EXEMPTION_THRESHOLD)
        .sum();
    let sweep_fee = MAX_DEPOSITS_PER_SWEEP as Lamport * FEE_PER_SIGNATURE;

    setup.advance_time(Duration::from_mins(1)).await;
    validator
        .wait_for_finalized_balance(
            &MINTER_ADDRESS,
            minter_sol_before + total_sweepable_amount - sweep_fee,
        )
        .await;

    let minter_transactions = validator.get_signatures_for_address(&MINTER_ADDRESS).await;
    let [swept_by] = minter_transactions.as_slice() else {
        panic!("Expected a single sweep transaction, got {minter_transactions:?}");
    };
    let sweep_signature = Signature::from(*swept_by);
    for deposit_address in &deposit_addresses {
        assert_eq!(
            validator.get_balance(deposit_address).await,
            RENT_EXEMPTION_THRESHOLD
        );
    }
    for &deposit_id in &deposit_ids {
        assert_eq!(
            setup.minter().deposit_status(deposit_id).await,
            DepositSolStatus::Swept {
                signature: sweep_signature.clone()
            }
        );
    }

    wait_for_minter_balance(&setup, total_sweepable_amount - sweep_fee).await;

    for &deposit_id in &deposit_ids {
        assert_eq!(
            setup.minter().deposit_status(deposit_id).await,
            DepositSolStatus::Finalized {
                signature: sweep_signature.clone()
            }
        );
    }

    for ((&deposit_id, &account), &deposit_amount) in
        deposit_ids.iter().zip(&accounts).zip(&deposit_amounts)
    {
        let expected_minted_amount = deposit_amount - RENT_EXEMPTION_THRESHOLD - FEE_PER_SIGNATURE;
        let minted_amount = setup.wait_for_deposit_minted(deposit_id).await;
        assert_eq!(minted_amount, expected_minted_amount);
        assert_eq!(
            setup.ledger().balance_of(account).await,
            expected_minted_amount
        );
    }

    setup.drop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn should_sweep_sub_rent_remainder_together_with_the_deposit() {
    const SUB_RENT_REMAINDER: Lamport = 500_000;

    let validator = SolanaTestValidator::start().await;
    let setup = validator.setup().await;
    let account = Account {
        owner: DEPOSITOR,
        subaccount: Some([0xAB; 32]),
    };
    let deposit_address: Address = setup.minter().get_deposit_address(account).await.into();

    validator
        .transfer_to(deposit_address, Setup::DEFAULT_MINIMUM_DEPOSIT_AMOUNT)
        .await;
    validator
        .transfer_to(deposit_address, SUB_RENT_REMAINDER)
        .await;
    let deposit_address_balance = Setup::DEFAULT_MINIMUM_DEPOSIT_AMOUNT + SUB_RENT_REMAINDER;
    validator
        .wait_for_finalized_balance(&deposit_address, deposit_address_balance)
        .await;

    let deposit_id = setup
        .minter()
        .deposit_sol(account)
        .await
        .expect("deposit_sol should queue a sweep");

    let sweepable_amount = deposit_address_balance - RENT_EXEMPTION_THRESHOLD;
    assert_eq!(
        setup.minter().deposit_status(deposit_id).await,
        DepositSolStatus::Queued { sweepable_amount }
    );

    setup.advance_time(Duration::from_mins(1)).await;
    let minted_amount = setup.wait_for_deposit_minted(deposit_id).await;

    assert_eq!(minted_amount, sweepable_amount - FEE_PER_SIGNATURE);
    assert_eq!(setup.ledger().balance_of(account).await, minted_amount);
    assert_eq!(
        validator.get_balance(&deposit_address).await,
        RENT_EXEMPTION_THRESHOLD
    );

    setup.drop().await;
}
