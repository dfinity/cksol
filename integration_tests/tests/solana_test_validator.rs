use assert_matches::assert_matches;
use candid::Principal;
use cksol_int_tests::{Setup, SetupBuilder, fixtures::MINTER_ADDRESS};
use cksol_types::{DepositStatus, UpdateBalanceArgs};
use icrc_ledger_types::icrc1::account::Account;
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

fn solana_test_setup() -> SetupBuilder {
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
}

#[tokio::test(flavor = "multi_thread")]
async fn should_deposit_and_consolidate_single_deposit() {
    let setup = solana_test_setup().build().await;

    // Bootstrap minter account
    airdrop_and_confirm(MINTER_ADDRESS, LAMPORTS_PER_SOL).await;

    let account = Account {
        owner: DEPOSITOR_PRINCIPAL,
        subaccount: Some([1; 32]),
    };
    let deposit_amount = LAMPORTS_PER_SOL / 10;

    let minter_cycles_before = setup.minter().cycle_balance().await;
    let minter_sol_before = get_solana_balance(&MINTER_ADDRESS).await;

    let deposit_address = deposit_to_account(&setup, account, deposit_amount).await;

    let minter_cycles_after_deposit = setup.minter().cycle_balance().await;

    // Consolidate
    setup.advance_time(Duration::from_mins(10)).await;
    tokio::time::sleep(Duration::from_secs(5)).await;

    // Deposit address should be drained after consolidation
    let balance_after = get_solana_balance(&deposit_address).await;
    assert_eq!(
        balance_after, 0,
        "Deposit address should be drained after consolidation"
    );

    let minter_sol_after = get_solana_balance(&MINTER_ADDRESS).await;
    let minter_cycles_after = setup.minter().cycle_balance().await;

    // The deposit fee should cover the consolidation tx fee so the minter's
    // SOL balance does not decrease.
    assert!(
        minter_sol_after >= minter_sol_before,
        "Minter SOL balance decreased: {minter_sol_before} -> {minter_sol_after}"
    );

    // The cycles charged during update_balance (deposit_consolidation_fee)
    // should more than offset the execution and signature costs of
    // consolidation, so the net balance does not drop below the initial one.
    assert!(
        minter_cycles_after >= minter_cycles_before,
        "Minter cycles balance decreased overall: {minter_cycles_before} -> {minter_cycles_after}"
    );

    setup.drop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn should_deposit_and_consolidate_funds() {
    const NUM_DEPOSITS: u8 = 15;

    let setup = solana_test_setup().build().await;

    // Bootstrap minter account
    airdrop_and_confirm(MINTER_ADDRESS, LAMPORTS_PER_SOL).await;

    let minter_cycles_before = setup.minter().cycle_balance().await;
    let minter_sol_before = get_solana_balance(&MINTER_ADDRESS).await;

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

    setup.advance_time(Duration::from_mins(10)).await;
    tokio::time::sleep(Duration::from_secs(5)).await;

    // Ensure the deposited funds were consolidated
    for (deposit_address, (&balance_before, &deposit_amount)) in deposit_addresses.iter().zip(
        deposit_account_balances_before_consolidation
            .iter()
            .zip(&deposit_amounts),
    ) {
        let balance_after = get_solana_balance(deposit_address).await;
        assert_eq!(balance_after, balance_before - deposit_amount);
    }

    let minter_sol_after = get_solana_balance(&MINTER_ADDRESS).await;
    let minter_cycles_after = setup.minter().cycle_balance().await;

    // The deposit fee (in lamports) should cover consolidation tx fees so the
    // minter's SOL balance does not decrease.
    assert!(
        minter_sol_after >= minter_sol_before,
        "Minter SOL balance decreased: {minter_sol_before} -> {minter_sol_after}"
    );

    // The cycles charged during update_balance (deposit_consolidation_fee per
    // deposit) should more than offset execution and signature costs of
    // consolidation, so the net balance does not drop below the initial one.
    assert!(
        minter_cycles_after >= minter_cycles_before,
        "Minter cycles balance decreased overall: {minter_cycles_before} -> {minter_cycles_after}"
    );

    setup.drop().await;
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
