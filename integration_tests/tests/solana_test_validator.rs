use candid::Principal;
use cksol_int_tests::SetupBuilder;
use cksol_types::{DepositStatus, GetDepositAddressArgs, UpdateBalanceArgs};
use sol_rpc_types::{InstallArgs, OverrideProvider, RegexSubstitution};
use solana_address::Address;
use solana_client::{rpc_client::RpcClient, rpc_config::CommitmentConfig};
use solana_keypair::{Keypair, Signer};
use solana_native_token::LAMPORTS_PER_SOL;
use solana_signature::Signature;

const SOLANA_VALIDATOR_URL: &str = "http://localhost:8899";
const PRINCIPAL: Principal = Principal::from_slice(&[0x9d, 0xf7, 0x99]);

#[tokio::test(flavor = "multi_thread")]
async fn should_update_balance_with_single_deposit() {
    const DEPOSIT_AMOUNT: u64 = 2 * LAMPORTS_PER_SOL;

    let setup = SetupBuilder::new()
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

    let deposit_address = setup
        .minter()
        .get_deposit_address(GetDepositAddressArgs {
            owner: Some(PRINCIPAL),
            subaccount: None,
        })
        .await
        .into();

    let deposit_signature = send_deposit_to_address(deposit_address, DEPOSIT_AMOUNT).await;

    let result = setup
        .minter()
        .update_balance(UpdateBalanceArgs {
            owner: Some(PRINCIPAL),
            subaccount: None,
            signature: deposit_signature.into(),
        })
        .await;

    // TODO DEFI-2643: Change once deposit logic is implemented
    assert_eq!(
        result,
        Ok(DepositStatus::Processing(deposit_signature.into()))
    );

    setup.drop().await;
}

async fn send_deposit_to_address(deposit_address: Address, deposit_amount: u64) -> Signature {
    let sender = Keypair::new();
    println!("Sender: {:?}", sender.pubkey());

    let rpc = RpcClient::new_with_commitment(
        SOLANA_VALIDATOR_URL.to_string(),
        CommitmentConfig::confirmed(),
    );

    // Fund sender with an airdrop
    let airdrop_amount = 2 * deposit_amount;
    let blockhash = rpc.get_latest_blockhash().unwrap();
    let airdrop_signature = rpc
        .request_airdrop_with_blockhash(&sender.pubkey(), airdrop_amount, &blockhash)
        .unwrap();
    rpc.confirm_transaction_with_spinner(&airdrop_signature, &blockhash, rpc.commitment())
        .unwrap();
    let balance = rpc.get_balance(&sender.pubkey()).unwrap();
    assert_eq!(balance, airdrop_amount);

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
