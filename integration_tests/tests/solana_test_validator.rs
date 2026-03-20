use assert_matches::assert_matches;
use candid::Principal;
use cksol_int_tests::{Setup, SetupBuilder, fixtures::MINTER_ADDRESS};
use cksol_types::{DepositStatus, UpdateBalanceArgs};
use cksol_types_internal::event::EventType;
use icrc_ledger_types::icrc1::account::Account;
use sol_rpc_types::Lamport;
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
const PROXY_PORT: u16 = 18899;

#[tokio::test(flavor = "multi_thread")]
async fn should_deposit_consolidate_and_resubmit() {
    use cksol_int_tests::json_rpc_reverse_proxy::JsonRpcRequestMatcher;

    const NUM_DEPOSITS: u8 = 15;

    let setup = SetupBuilder::new()
        .with_proxy_canister()
        .with_pocket_ic_live_mode()
        .with_json_rpc_proxy(SOLANA_VALIDATOR_URL, PROXY_PORT)
        .build()
        .await;

    airdrop_and_confirm(MINTER_ADDRESS, LAMPORTS_PER_SOL).await;

    // Create deposits
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

    // Trigger consolidation and verify funds are moved
    let balances_before = get_balances(&deposit_addresses).await;
    let minter_balance_before = get_solana_balance(&MINTER_ADDRESS).await;

    setup.advance_time(Duration::from_mins(10)).await;
    tokio::time::sleep(Duration::from_secs(5)).await;

    for (addr, (&before, &amount)) in deposit_addresses
        .iter()
        .zip(balances_before.iter().zip(&deposit_amounts))
    {
        assert_eq!(get_solana_balance(addr).await, before - amount);
    }
    assert_eq!(
        get_solana_balance(&MINTER_ADDRESS).await,
        minter_balance_before + deposit_amounts.iter().sum::<Lamport>()
            - consolidation_transaction_fees(NUM_DEPOSITS as u64)
    );

    // Block sendTransaction and create new deposits for the resubmission test
    setup
        .json_rpc_proxy()
        .block(JsonRpcRequestMatcher::with_method("sendTransaction"))
        .await;

    let (resubmit_addresses, resubmit_amounts): (Vec<_>, Vec<_>) =
        futures::future::join_all((1_u8..=3).map(async |i| {
            let account = Account {
                owner: DEPOSITOR_PRINCIPAL,
                subaccount: Some([i + 100; 32]),
            };
            let deposit_amount = (i as u64 * LAMPORTS_PER_SOL) / 10;
            let deposit_address = deposit_to_account(&setup, account, deposit_amount).await;
            (deposit_address, deposit_amount)
        }))
        .await
        .into_iter()
        .unzip();

    let resubmit_balances_before = get_balances(&resubmit_addresses).await;
    let minter_balance_before_resubmit = get_solana_balance(&MINTER_ADDRESS).await;

    // Trigger consolidation — transaction is "submitted" but dropped by the proxy
    setup.advance_time(Duration::from_mins(10)).await;
    tokio::time::sleep(Duration::from_secs(3)).await;

    setup.json_rpc_proxy().clear_blocklist().await;

    // Funds should NOT be consolidated yet
    assert_eq!(
        resubmit_balances_before,
        get_balances(&resubmit_addresses).await,
    );

    // Wait for the transaction to expire (150 slots ≈ 60s) and the resubmission timer to fire
    tokio::time::sleep(Duration::from_secs(130)).await;

    let events = setup.minter().get_all_events().await;
    assert!(
        events
            .iter()
            .any(|e| matches!(e.payload, EventType::ResubmittedTransaction { .. }))
    );

    // Wait for the resubmitted transaction to confirm
    tokio::time::sleep(Duration::from_secs(15)).await;

    // Verify funds are now consolidated
    for (addr, (&before, &amount)) in resubmit_addresses
        .iter()
        .zip(resubmit_balances_before.iter().zip(&resubmit_amounts))
    {
        assert_eq!(get_solana_balance(addr).await, before - amount);
    }
    assert_eq!(
        get_solana_balance(&MINTER_ADDRESS).await,
        minter_balance_before_resubmit + resubmit_amounts.iter().sum::<Lamport>()
            - consolidation_transaction_fees(3)
    );

    setup.drop().await;
}

fn consolidation_transaction_fees(num_deposits: u64) -> Lamport {
    const MAX_ACCOUNTS_PER_CONSOLIDATION_TRANSACTION: u64 = 9;
    let num_transactions = num_deposits.div_ceil(MAX_ACCOUNTS_PER_CONSOLIDATION_TRANSACTION);
    (num_transactions + num_deposits) * FEE_PER_SIGNATURE
}

async fn deposit_to_account(setup: &Setup, account: Account, amount: Lamport) -> Address {
    let expected_mint_amount = amount - Setup::DEFAULT_DEPOSIT_FEE;
    let deposit_address = setup.minter().get_deposit_address(account).await.into();

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
        signature,
        block_index: _,
    }) if minted_amount == expected_mint_amount && signature == deposit_signature.into());

    let balance_after = setup.ledger().balance_of(account).await;
    assert_eq!(balance_after, expected_mint_amount);

    deposit_address
}

async fn send_deposit_to_address(deposit_address: Address, deposit_amount: Lamport) -> Signature {
    let sender = Keypair::new();
    airdrop_and_confirm(sender.pubkey(), 2 * deposit_amount).await;

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
    assert_eq!(
        rpc.get_balance(&address).await.unwrap(),
        balance_before + airdrop_amount
    );
}

async fn confirm_transaction(rpc: &RpcClient, signature: &Signature, commitment: CommitmentConfig) {
    for _ in 0..60 {
        if let Ok(result) = rpc
            .confirm_transaction_with_commitment(signature, commitment)
            .await
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
