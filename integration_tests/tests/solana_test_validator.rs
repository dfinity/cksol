use assert_matches::assert_matches;
use candid::Principal;
use cksol_int_tests::{
    Setup, SetupBuilder, fixtures::MINTER_ADDRESS, ledger_init_args::LEDGER_TRANSFER_FEE,
};
use cksol_types::{DepositStatus, UpdateBalanceArgs, WithdrawSolArgs, WithdrawSolStatus};
use icrc_ledger_types::icrc1::account::Account;
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

// Solana fee per transaction signature
const FEE_PER_SIGNATURE: Lamport = 5_000;

/// Creates a test setup connected to the local Solana test validator
/// and airdrops SOL to the minter so it can pay for transaction fees.
async fn setup_with_solana_validator() -> Setup {
    let setup = SetupBuilder::new()
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
        .await;

    airdrop_and_confirm(MINTER_ADDRESS, LAMPORTS_PER_SOL).await;

    setup
}

#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn should_deposit_and_consolidate_funds() {
    const NUM_DEPOSITS: u8 = 15;

    let setup = setup_with_solana_validator().await;

    let (deposit_addresses, deposit_amounts): (Vec<_>, Vec<_>) =
        futures::future::join_all((1_u8..=NUM_DEPOSITS).map(async |i| {
            let account = Account {
                owner: DEPOSITOR_PRINCIPAL,
                subaccount: Some([i; 32]),
            };
            let deposit_amount = (i as u64 * LAMPORTS_PER_SOL) / 10;
            let deposit_address = deposit_to_account(&setup, account, deposit_amount).await;
            (deposit_address, deposit_amount)
        }))
        .await
        .into_iter()
        .unzip();

    // Check deposit consolidation
    let deposit_account_balances_before_consolidation = get_balances(&deposit_addresses).await;
    let minter_balance_before_consolidation = get_solana_balance(&MINTER_ADDRESS).await;

    setup.advance_time(Duration::from_mins(10)).await;
    tokio::time::sleep(Duration::from_secs(5)).await;

    // Ensure the deposited funds were consolidated, note that we do not assert the balance after
    // consolidation to be zero due to potential funds leftover from previous tests
    for (deposit_address, (&balance_before, &deposit_amount)) in deposit_addresses.iter().zip(
        deposit_account_balances_before_consolidation
            .iter()
            .zip(&deposit_amounts),
    ) {
        let balance_after = get_solana_balance(deposit_address).await;
        assert_eq!(balance_after, balance_before - deposit_amount);
    }
    let minter_balance_after_consolidation = get_solana_balance(&MINTER_ADDRESS).await;
    assert_eq!(
        minter_balance_after_consolidation,
        minter_balance_before_consolidation + deposit_amounts.iter().sum::<Lamport>()
            - consolidation_transaction_fees(NUM_DEPOSITS as u64)
    );

    setup.drop().await;
}

fn consolidation_transaction_fees(num_deposits: u64) -> Lamport {
    // Maximum number of transfer instructions per consolidation transaction
    const MAX_ACCOUNTS_PER_CONSOLIDATION_TRANSACTION: u64 = 9;
    let num_transactions = num_deposits.div_ceil(MAX_ACCOUNTS_PER_CONSOLIDATION_TRANSACTION);
    // Total signatures = num_transactions (fee payers) + num_deposits (sources)
    (num_transactions + num_deposits) * FEE_PER_SIGNATURE
}

async fn deposit_to_account(setup: &Setup, account: Account, amount: Lamport) -> Address {
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

    deposit_address
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

async fn get_solana_balance(address: &Address) -> Lamport {
    rpc_client()
        .get_balance(address)
        .await
        .expect("Failed to get Solana balance")
}

fn rpc_client() -> RpcClient {
    RpcClient::new_with_commitment(
        SOLANA_VALIDATOR_URL.to_string(),
        CommitmentConfig::confirmed(),
    )
}

async fn get_balances(addresses: &[Address]) -> Vec<Lamport> {
    futures::future::join_all(addresses.iter().map(get_solana_balance)).await
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

    // Step 2: Withdraw ckSOL back to a fresh Solana address
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
        .withdraw_sol(WithdrawSolArgs {
            from_subaccount: depositor.subaccount,
            amount: withdrawal_amount,
            address: withdrawal_address.to_string(),
        })
        .await
        .expect("withdraw_sol should succeed");

    let burn_index = withdraw_result.block_index;

    // Verify ckSOL was burned (withdrawal amount + ledger transfer fee)
    let ck_balance_after = setup.ledger().balance_of(depositor).await;
    assert_eq!(
        ck_balance_after,
        expected_mint_amount - withdrawal_amount - LEDGER_TRANSFER_FEE
    );

    // Step 3: Advance time to trigger withdrawal processing
    setup.advance_time(Duration::from_mins(2)).await;
    tokio::time::sleep(Duration::from_secs(5)).await;

    // Step 4: Verify the withdrawal was sent
    let status = setup.minter().withdraw_sol_status(burn_index).await;
    assert_matches!(status, WithdrawSolStatus::TxSent(_));

    // Step 5: Wait for the transaction to be confirmed on Solana
    let tx_hash = match setup.minter().withdraw_sol_status(burn_index).await {
        WithdrawSolStatus::TxSent(tx) => tx.transaction_hash,
        other => panic!("Expected TxSent, got: {other:?}"),
    };
    let tx_signature: Signature = tx_hash.parse().expect("valid signature");
    confirm_transaction(&rpc_client(), &tx_signature, CommitmentConfig::confirmed()).await;

    // Step 6: Verify the destination received the funds
    let destination_balance = get_solana_balance(&withdrawal_address).await;
    let expected_received = withdrawal_amount - Setup::DEFAULT_WITHDRAWAL_FEE;
    assert_eq!(destination_balance, expected_received);

    setup.drop().await;
}
