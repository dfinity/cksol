use assert_matches::assert_matches;
use candid::Principal;
use cksol_int_tests::{
    Setup, SetupBuilder, fixtures::MINTER_ADDRESS, ledger_init_args::LEDGER_TRANSFER_FEE,
};
use cksol_types::{DepositStatus, UpdateBalanceArgs, WithdrawalArgs, WithdrawalStatus};
use icrc_ledger_types::icrc1::account::Account;
use itertools::Itertools;
use sol_rpc_types::{InstallArgs, Lamport, OverrideProvider, RegexSubstitution};
use solana_address::Address;
use solana_client::{nonblocking::rpc_client::RpcClient, rpc_config::CommitmentConfig};
use solana_keypair::{Keypair, Signer};
use solana_native_token::LAMPORTS_PER_SOL;
use solana_signature::Signature;
use std::time::Duration;

const SOLANA_VALIDATOR_URL: &str = "http://localhost:8899";
const DEPOSITOR: Principal = Setup::DEFAULT_CALLER;
// Solana base fee per signature included in a transaction.
const FEE_PER_SIGNATURE: Lamport = 5_000;

// TODO DEFI-2643: Add tests with more exotic transactions, e.g.:
//  - a transaction with multiple transfer instructions to same target address: single mint with the summed up amount
//  - a transaction with multiple instructions, not all to the same target address: only relevant amounts are considered.

#[tokio::test(flavor = "multi_thread")]
async fn should_deposit_consolidate_and_withdraw() {
    let setup = setup_with_solana_validator().await;

    let withdrawal_destination = Keypair::new();
    let withdrawal_address = withdrawal_destination.pubkey();

    for (i, num_deposits) in [1_u8, 15].into_iter().enumerate() {
        println!("Testing with {num_deposits} deposit(s)");

        let minter_cycles_before = setup.minter().cycle_balance().await;
        let minter_sol_before = get_solana_balance(&MINTER_ADDRESS).await;
        let destination_sol_before = get_solana_balance(&withdrawal_address).await;

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
                let (deposit_address, minted_amount) =
                    deposit_to_account(&setup, *account, deposit_amount).await;
                (deposit_address, deposit_amount, minted_amount)
            }))
            .await
            .into_iter()
            .multiunzip();

        let total_minted_amount = minted_amounts.iter().sum::<Lamport>();
        let total_deposited_amount = deposit_amounts.iter().sum::<Lamport>();

        let deposit_accounts_balances_before = get_balances(&deposit_addresses).await;

        // Trigger consolidation and wait for the minter's Solana balance to increase
        setup.advance_time(Duration::from_mins(10)).await;
        wait_for_finalized_balance(&MINTER_ADDRESS, minter_sol_before).await;

        // Verify deposit addresses were drained
        for (deposit_address, &balance_before, &deposit_amount) in itertools::multizip((
            &deposit_addresses,
            &deposit_accounts_balances_before,
            &deposit_amounts,
        )) {
            let balance_after = get_solana_balance(deposit_address).await;
            assert_eq!(balance_after, balance_before - deposit_amount);
        }

        let minter_sol_after_consolidation = get_solana_balance(&MINTER_ADDRESS).await;
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

        // Wait for all withdrawals to be finalized (in parallel)
        futures::future::join_all(
            burn_indices
                .iter()
                .map(|&idx| wait_for_withdrawal_finalized(&setup, idx)),
        )
        .await;

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
        let destination_sol_after = get_solana_balance(&withdrawal_address).await;
        assert_eq!(
            destination_sol_after - destination_sol_before,
            expected_received
        );

        // Minter should retain at least its initial SOL balance (withdrawal fees stay with it)
        let minter_sol_final = get_solana_balance(&MINTER_ADDRESS).await;
        assert!(
            minter_sol_final >= minter_sol_before,
            "Minter SOL balance should not decrease"
        );
    }

    setup.drop().await;
}

/// Creates a test setup connected to the local Solana test validator.
async fn setup_with_solana_validator() -> Setup {
    SetupBuilder::new()
        .with_proxy_canister()
        .with_pocket_ic_live_mode()
        .with_sol_rpc_install_args(InstallArgs {
            override_provider: Some(OverrideProvider {
                override_url: Some(RegexSubstitution {
                    pattern: ".*".into(),
                    replacement: SOLANA_VALIDATOR_URL.to_string(),
                }),
            }),
            ..InstallArgs::default()
        })
        .build()
        .await
}

async fn deposit_to_account(
    setup: &Setup,
    account: Account,
    amount: Lamport,
) -> (Address, Lamport) {
    let expected_mint_amount = amount - Setup::DEFAULT_DEPOSIT_FEE;
    let deposit_address = setup.minter().get_deposit_address(account).await.into();

    println!("Depositing {amount} Lamport to address {deposit_address}");

    let balance_before = setup.ledger().balance_of(account).await;
    assert_eq!(balance_before, 0);

    let deposit_signature = send_deposit_to_address(deposit_address, amount).await;

    let result = setup
        .minter()
        .update_balance(UpdateBalanceArgs {
            owner: Some(account.owner),
            subaccount: account.subaccount,
            signature: deposit_signature.into(),
        })
        .await;
    assert_matches!(result, Ok(DepositStatus::Minted {
        minted_amount,
        deposit_id,
        block_index: _,
    }) if minted_amount == expected_mint_amount
        && deposit_id.signature == deposit_signature.into()
        && deposit_id.account == account);

    let balance_after = setup.ledger().balance_of(account).await;
    assert_eq!(balance_after, expected_mint_amount);

    (deposit_address, expected_mint_amount)
}

async fn send_deposit_to_address(deposit_address: Address, deposit_amount: Lamport) -> Signature {
    let sender = Keypair::new();

    // Fund sender with an airdrop
    airdrop_and_confirm(sender.pubkey(), 2 * deposit_amount).await;

    // Build and submit deposit transaction
    let rpc = rpc_client();
    let recent_blockhash = rpc.get_latest_blockhash().await.unwrap();
    let transaction = solana_system_transaction::transfer(
        &sender,
        &deposit_address,
        deposit_amount,
        recent_blockhash,
    );
    let signature = rpc.send_transaction(&transaction).await.unwrap();
    confirm_transaction(&rpc, &signature, CommitmentConfig::finalized()).await;
    signature
}

async fn airdrop_and_confirm(address: Address, airdrop_amount: Lamport) {
    let rpc = rpc_client();

    let balance_before = rpc.get_balance(&address).await.unwrap();

    let blockhash = rpc.get_latest_blockhash().await.unwrap();
    let airdrop_signature = rpc
        .request_airdrop_with_blockhash(&address, airdrop_amount, &blockhash)
        .await
        .unwrap();
    confirm_transaction(&rpc, &airdrop_signature, CommitmentConfig::confirmed()).await;

    let balance_after = rpc.get_balance(&address).await.unwrap();
    assert_eq!(balance_after, balance_before + airdrop_amount);
}

async fn confirm_transaction(rpc: &RpcClient, signature: &Signature, commitment: CommitmentConfig) {
    for _ in 0..60 {
        let response = rpc
            .confirm_transaction_with_commitment(signature, commitment)
            .await;
        if let Ok(result) = response
            && result.value
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    panic!("Transaction {signature} not confirmed within timeout");
}

/// Polls a Solana address at `finalized` commitment until its balance exceeds
/// `previous_balance`.
async fn wait_for_finalized_balance(address: &Address, previous_balance: Lamport) {
    for _ in 0..60 {
        let balance = rpc_client()
            .get_balance_with_commitment(address, CommitmentConfig::finalized())
            .await
            .map(|response| response.value)
            .unwrap_or(0);
        if balance > previous_balance {
            return;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    panic!(
        "Balance of {address} did not increase beyond {previous_balance} at finalized commitment"
    );
}

/// Polls the minter until the given withdrawal is finalized.
/// In live mode, PocketIC auto-advances time and fires timers automatically.
async fn wait_for_withdrawal_finalized(setup: &Setup, burn_index: u64) {
    for _ in 0..120 {
        if matches!(
            setup.minter().withdrawal_status(burn_index).await,
            WithdrawalStatus::TxFinalized(_)
        ) {
            return;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    panic!("Withdrawal {burn_index} did not finalize within timeout");
}

async fn get_solana_balance(address: &Address) -> Lamport {
    rpc_client()
        .get_balance(address)
        .await
        .expect("Failed to get Solana balance")
}

async fn get_balances(addresses: &[Address]) -> Vec<Lamport> {
    futures::future::join_all(addresses.iter().map(get_solana_balance)).await
}

fn rpc_client() -> RpcClient {
    RpcClient::new_with_commitment(
        SOLANA_VALIDATOR_URL.to_string(),
        CommitmentConfig::confirmed(),
    )
}
