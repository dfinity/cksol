use candid::Principal;
use cksol_int_tests::{Setup, SetupBuilder};
use cksol_types::GetDepositAddressArgs;
use solana_address::{Address, address};

const DEPOSIT_ADDRESS: Address = address!("Ge2aoiaTb6Tq2DQ4xs7qGhGud97pKtDmJCAQufTJeNSu");

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
            subaccount: None,
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
            owner: None,
            subaccount: Some(subaccount),
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
            owner: None,
            subaccount: Some(subaccount1),
        })
        .await;

    let address2 = setup
        .minter()
        .get_deposit_address(GetDepositAddressArgs {
            owner: None,
            subaccount: Some(subaccount2),
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
            subaccount: None,
        })
        .await;

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.contains("the owner must be non-anonymous"));
}

#[tokio::test]
async fn should_succeed_for_anonymous_caller_with_owner() {
    let setup = SetupBuilder::new()
        .with_caller(Principal::anonymous())
        .build()
        .await;

    let deposit_address = setup
        .minter()
        .get_deposit_address(GetDepositAddressArgs {
            owner: Some(Setup::DEFAULT_CALLER),
            subaccount: None,
        })
        .await;

    assert_eq!(Address::from(deposit_address), DEPOSIT_ADDRESS);
}
