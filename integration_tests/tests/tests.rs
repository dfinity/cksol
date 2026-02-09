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
