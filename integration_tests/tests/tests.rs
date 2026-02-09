use candid::Principal;
use cksol_int_tests::SetupBuilder;
use cksol_types::GetDepositAddressArgs;
use solana_address::{Address, address};

const DEPOSIT_ADDRESS: Address = address!("4Ddk4XxD8nwnnMApEAdJXLG3nf9UEvrDEP6B5bYZyzwn");

#[tokio::test]
async fn should_get_deposit_address() {
    let setup = SetupBuilder::new().build().await;

    let deposit_address = setup
        .minter()
        .get_deposit_address(GetDepositAddressArgs::default())
        .await;

    assert_eq!(Address::from(deposit_address), DEPOSIT_ADDRESS);
}

#[tokio::test]
async fn should_get_deposit_address_for_explicit_owner() {
    let setup = SetupBuilder::new().build().await;
    // Using a different principal than the default caller
    let owner = Principal::from_slice(&[1]);

    let deposit_address = setup
        .minter()
        .get_deposit_address(GetDepositAddressArgs {
            owner: Some(owner),
            ..Default::default()
        })
        .await;

    assert_ne!(Address::from(deposit_address), DEPOSIT_ADDRESS);
}

#[tokio::test]
async fn should_get_deposit_address_with_subaccount() {
    let setup = SetupBuilder::new().build().await;
    let subaccount = [1; 32];

    let deposit_address = setup
        .minter()
        .get_deposit_address(GetDepositAddressArgs {
            subaccount: Some(subaccount),
            ..Default::default()
        })
        .await;

    assert_ne!(Address::from(deposit_address), DEPOSIT_ADDRESS);
}

#[tokio::test]
async fn should_get_different_addresses_for_different_subaccounts() {
    let setup = SetupBuilder::new().build().await;
    let subaccount1 = [1; 32];
    let subaccount2 = [2; 32];

    let address1 = setup
        .minter()
        .get_deposit_address(GetDepositAddressArgs {
            subaccount: Some(subaccount1),
            ..Default::default()
        })
        .await;

    let address2 = setup
        .minter()
        .get_deposit_address(GetDepositAddressArgs {
            subaccount: Some(subaccount2),
            ..Default::default()
        })
        .await;

    assert_ne!(address1, address2);
}

#[tokio::test]
async fn should_fail_for_anonymous_owner() {
    let setup = SetupBuilder::new().build().await;

    let result = setup
        .minter()
        .get_deposit_address_result(GetDepositAddressArgs {
            owner: Some(Principal::anonymous()),
            ..Default::default()
        })
        .await;

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.contains("the owner must be non-anonymous"));
}
