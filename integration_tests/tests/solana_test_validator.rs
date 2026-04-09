use assert_matches::assert_matches;
use candid::Principal;
use cksol_int_tests::{
    Setup, SetupBuilder, fixtures::MINTER_ADDRESS, ledger_init_args::LEDGER_TRANSFER_FEE,
};
use cksol_types::{DepositStatus, UpdateBalanceArgs, WithdrawalArgs, WithdrawalStatus};
use icrc_ledger_types::icrc1::account::Account;
use itertools::Itertools;
use serial_test::serial;
use sol_rpc_types::{InstallArgs, Lamport, OverrideProvider, RegexSubstitution};
use solana_address::Address;
use solana_client::{nonblocking::rpc_client::RpcClient, rpc_config::CommitmentConfig};
use solana_keypair::{Keypair, Signer};
use solana_native_token::LAMPORTS_PER_SOL;
use solana_signature::Signature;
use std::time::Duration;

const SOLANA_VALIDATOR_URL: &str = "http://localhost:8899";
const DEPOSITOR_PRINCIPAL: Principal = Principal::from_slice(&[0x9d, 0xf7, 0x99]);

// TODO DEFI-2643: Add tests with more exotic transactions, e.g.:
//  - a transaction with multiple transfer instructions to same target address: single mint with the summed up amount
//  - a transaction with multiple instructions, not all to the same target address: only relevant amounts are considered.

#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn should_deposit_and_consolidate_funds() {
    let setup = setup_with_solana_validator().await;

    for num_deposits in [1_u8, 15] {
        println!("Testing with {num_deposits} deposit(s)");

        let minter_cycles_before = setup.minter().cycle_balance().await;
        let minter_sol_before = get_solana_balance(&MINTER_ADDRESS).await;

        let (deposit_addresses, deposit_amounts, minted_amounts): (Vec<_>, Vec<_>, Vec<_>) =
            futures::future::join_all((1_u8..=num_deposits).map(async |i| {
                let account = Account {
                    owner: DEPOSITOR_PRINCIPAL,
                    subaccount: Some({
                        let mut sub = [0u8; 32];
                        sub[0] = num_deposits;
                        sub[1] = i;
                        sub
                    }),
                };
                let deposit_amount = (i as u64 * LAMPORTS_PER_SOL) / 10;
                let (deposit_address, minted_amount) =
                    deposit_to_account(&setup, account, deposit_amount).await;
                (deposit_address, deposit_amount, minted_amount)
            }))
            .await
            .into_iter()
            .multiunzip();

        let deposit_accounts_balances_before = get_balances(&deposit_addresses).await;

        // Trigger consolidation and wait for the transaction to finalize
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

        let total_minted_amount = minted_amounts.iter().sum::<Lamport>();

        let minter_sol_after = get_solana_balance(&MINTER_ADDRESS).await;
        let minter_cycles_after = setup.minter().cycle_balance().await;

        let minter_sol_change = minter_sol_after as i64 - minter_sol_before as i64;
        let minter_cycles_change = minter_cycles_after as i128 - minter_cycles_before as i128;
        println!(
            "  SOL balance change minus total minted amount: {} lamports",
            minter_sol_change - total_minted_amount as i64
        );
        println!("  Cycles balance change: {minter_cycles_change}");

        assert!(
            minter_sol_after >= minter_sol_before + total_minted_amount,
            "Minter SOL balance increased less than the minted amount"
        );
        assert!(
            minter_cycles_after >= minter_cycles_before,
            "Minter cycles balance decreased"
        );
    }

    setup.drop().await;
}

#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn should_deposit_and_withdraw_funds() {
    let setup = setup_with_solana_validator().await;

    let depositor = Account {
        owner: Setup::DEFAULT_CALLER,
        subaccount: None,
    };
    let deposit_amount = LAMPORTS_PER_SOL;
    let expected_mint_amount = deposit_amount - Setup::DEFAULT_DEPOSIT_FEE;

    // Step 1: Deposit SOL and mint ckSOL
    let deposit_address = setup.minter().get_deposit_address(depositor).await.into();
    let deposit_signature = send_deposit_to_address(deposit_address, deposit_amount).await;

    let result = setup
        .minter()
        .update_balance(UpdateBalanceArgs {
            owner: Some(depositor.owner),
            subaccount: depositor.subaccount,
            signature: deposit_signature.into(),
        })
        .await;
    assert_matches!(result, Ok(DepositStatus::Minted { .. }));

    let ck_balance = setup.ledger().balance_of(depositor).await;
    assert_eq!(ck_balance, expected_mint_amount);

    // Step 2: Consolidate the deposit so the minter has on-chain SOL.
    // Wait for the consolidation to be finalized, because sendTransaction's
    // preflight simulation defaults to finalized commitment.
    let minter_sol_before = get_solana_balance(&MINTER_ADDRESS).await;
    setup.advance_time(Duration::from_mins(10)).await;
    wait_for_finalized_balance(&MINTER_ADDRESS, minter_sol_before).await;

    // Step 3: Withdraw ckSOL back to a fresh Solana address
    let withdrawal_destination = Keypair::new();
    let withdrawal_address = withdrawal_destination.pubkey();
    let withdrawal_amount = expected_mint_amount / 2;

    // Approve minter to spend ckSOL
    setup
        .ledger()
        .approve(
            depositor.subaccount,
            withdrawal_amount,
            Account {
                owner: setup.minter_canister_id(),
                subaccount: None,
            },
        )
        .await;

    // Initiate withdrawal
    let withdraw_result = setup
        .minter()
        .withdraw(WithdrawalArgs {
            from_subaccount: depositor.subaccount,
            amount: withdrawal_amount,
            address: withdrawal_address.to_string(),
        })
        .await
        .expect("withdraw should succeed");

    let burn_index = withdraw_result.block_index;

    // Verify ckSOL was burned (withdrawal amount + ledger transfer fee)
    let ck_balance_after = setup.ledger().balance_of(depositor).await;
    assert_eq!(
        ck_balance_after,
        expected_mint_amount - withdrawal_amount - LEDGER_TRANSFER_FEE
    );

    // Step 4: Advance time to trigger withdrawal processing
    setup.advance_time(Duration::from_mins(2)).await;
    tokio::time::sleep(Duration::from_secs(5)).await;

    // Step 5: Verify the withdrawal was sent
    let tx_hash = match setup.minter().withdrawal_status(burn_index).await {
        WithdrawalStatus::TxSent(tx) => tx.transaction_hash,
        other => panic!("Expected TxSent, got: {other:?}"),
    };

    // Step 6: Wait for the transaction to be confirmed on Solana
    let tx_signature: Signature = tx_hash.parse().expect("valid signature");
    confirm_transaction(&rpc_client(), &tx_signature, CommitmentConfig::confirmed()).await;

    // Step 7: Verify the destination received the funds
    let destination_balance = get_solana_balance(&withdrawal_address).await;
    let expected_received = withdrawal_amount - Setup::DEFAULT_WITHDRAWAL_FEE;
    assert_eq!(destination_balance, expected_received);

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
