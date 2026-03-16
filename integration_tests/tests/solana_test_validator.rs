use assert_matches::assert_matches;
use candid::Principal;
use cksol_int_tests::fixtures::MINTER_ADDRESS;
use cksol_int_tests::{Setup, SetupBuilder};
use cksol_types::{DepositStatus, UpdateBalanceArgs};
use icrc_ledger_types::icrc1::account::Account;
use sol_rpc_types::{InstallArgs, Lamport, OverrideProvider, RegexSubstitution};
use solana_address::Address;
use solana_client::{rpc_client::RpcClient, rpc_config::CommitmentConfig};
use solana_keypair::{Keypair, Signer};
use solana_native_token::LAMPORTS_PER_SOL;
use solana_signature::Signature;
use std::time::Duration;

const SOLANA_VALIDATOR_URL: &str = "http://localhost:8899";
const DEPOSITOR_PRINCIPAL: Principal = Principal::from_slice(&[0x9d, 0xf7, 0x99]);
const DEPOSITOR_ACCOUNT: Account = Account {
    owner: DEPOSITOR_PRINCIPAL,
    subaccount: None,
};

// TODO DEFI-2643: Add tests with more exotic transactions, e.g.:
//  - a transaction with multiple transfer instructions to same target address: single mint with the summed up amount
//  - a transaction with multiple instructions, not all to the same target address: only relevant amounts are considered.

// Solana fee per transaction signature
const FEE_PER_SIGNATURE: Lamport = 5_000;

#[tokio::test(flavor = "multi_thread")]
async fn should_update_balance_and_consolidate_funds() {
    const DEPOSIT_AMOUNT: Lamport = LAMPORTS_PER_SOL / 10;
    const EXPECTED_MINT_AMOUNT: Lamport = DEPOSIT_AMOUNT - Setup::DEFAULT_DEPOSIT_FEE;

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

    // Bootstrap minter account
    airdrop_to_address(MINTER_ADDRESS, LAMPORTS_PER_SOL);

    let deposit_address = setup
        .minter()
        .get_deposit_address(DEPOSITOR_ACCOUNT)
        .await
        .into();

    let balance_before = setup.ledger().balance_of(DEPOSITOR_ACCOUNT).await;
    assert_eq!(balance_before, 0);

    let deposit_signature = send_deposit_to_address(deposit_address, DEPOSIT_AMOUNT).await;

    let result = setup
        .minter()
        .update_balance(UpdateBalanceArgs {
            owner: Some(DEPOSITOR_PRINCIPAL),
            subaccount: None,
            signature: deposit_signature.into(),
        })
        .await;
    assert_matches!(result, Ok(DepositStatus::Minted {
        minted_amount,
        signature,
        block_index: _,
    }) if minted_amount == EXPECTED_MINT_AMOUNT && signature == deposit_signature.into());

    let balance_after = setup.ledger().balance_of(DEPOSITOR_ACCOUNT).await;
    assert_eq!(balance_after, EXPECTED_MINT_AMOUNT);

    // Check deposit consolidation
    let deposit_account_balance_before_consolidation = get_solana_balance(&deposit_address).await;
    let minter_balance_before_consolidation = get_solana_balance(&MINTER_ADDRESS).await;

    setup.advance_time(Duration::from_mins(10)).await;
    tokio::time::sleep(Duration::from_secs(5)).await;

    let deposit_account_balance_after_consolidation = get_solana_balance(&deposit_address).await;
    let minter_balance_after_consolidation = get_solana_balance(&MINTER_ADDRESS).await;

    assert_eq!(
        deposit_account_balance_after_consolidation,
        deposit_account_balance_before_consolidation - DEPOSIT_AMOUNT
    );
    assert_eq!(
        minter_balance_after_consolidation,
        minter_balance_before_consolidation + DEPOSIT_AMOUNT - (2 * FEE_PER_SIGNATURE)
    );

    setup.drop().await;
}

async fn send_deposit_to_address(deposit_address: Address, deposit_amount: Lamport) -> Signature {
    let sender = Keypair::new();

    let rpc = RpcClient::new_with_commitment(
        SOLANA_VALIDATOR_URL.to_string(),
        CommitmentConfig::confirmed(),
    );

    // Fund sender with an airdrop
    airdrop_to_address(sender.pubkey(), 2 * deposit_amount);

    // Build and submit deposit transaction
    let recent_blockhash = rpc.get_latest_blockhash().unwrap();
    let transaction = solana_system_transaction::transfer(
        &sender,
        &deposit_address,
        deposit_amount,
        recent_blockhash,
    );
    let signature = rpc.send_and_confirm_transaction(&transaction).unwrap();
    rpc.confirm_transaction_with_spinner(
        &signature,
        &recent_blockhash,
        CommitmentConfig::finalized(),
    )
    .unwrap();
    signature
}

fn airdrop_to_address(address: Address, airdrop_amount: Lamport) {
    let rpc = RpcClient::new_with_commitment(
        SOLANA_VALIDATOR_URL.to_string(),
        CommitmentConfig::confirmed(),
    );

    let balance_before = rpc.get_balance(&address).unwrap();

    let blockhash = rpc.get_latest_blockhash().unwrap();
    let airdrop_signature = rpc
        .request_airdrop_with_blockhash(&address, airdrop_amount, &blockhash)
        .unwrap();
    rpc.confirm_transaction_with_spinner(&airdrop_signature, &blockhash, rpc.commitment())
        .unwrap();

    let balance_after = rpc.get_balance(&address).unwrap();
    assert_eq!(balance_after, balance_before + airdrop_amount);
}

async fn get_solana_balance(address: &Address) -> Lamport {
    RpcClient::new_with_commitment(
        SOLANA_VALIDATOR_URL.to_string(),
        CommitmentConfig::confirmed(),
    )
    .get_balance(address)
    .expect("Failed to get Solana balance")
}
