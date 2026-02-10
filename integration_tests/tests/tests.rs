mod get_deposit_address_tests {
    use candid::Principal;
    use cksol_int_tests::{Setup, SetupBuilder};
    use cksol_types::GetDepositAddressArgs;

    const DEFAULT_CALLER_DEPOSIT_ADDRESS: &str = "Ge2aoiaTb6Tq2DQ4xs7qGhGud97pKtDmJCAQufTJeNSu";

    #[tokio::test]
    async fn should_get_deposit_address_for_default_owner() {
        let setup = SetupBuilder::new().build().await;

        let deposit_address = setup
            .minter()
            .get_deposit_address(GetDepositAddressArgs::default())
            .await;

        assert_eq!(deposit_address.to_string(), DEFAULT_CALLER_DEPOSIT_ADDRESS);
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

        assert_eq!(
            deposit_address.to_string(),
            "9qvNPGSFQY8fvmr5A2jyCmSBfN7rrWBGJEAGgpN2TKeV"
        );
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
        assert_eq!(
            address1.to_string(),
            "97eLNQ1sc7yQHscLWet7vq7AZ6TbxN5nx8D8LPSbYEJB"
        );

        let address2 = setup
            .minter()
            .get_deposit_address(GetDepositAddressArgs {
                owner: None,
                subaccount: Some(subaccount2),
            })
            .await;

        assert_eq!(
            address2.to_string(),
            "BiuUj1yMbtStuumWutpBajSjNDPbnE5dNEuTv7J1cjmB"
        );
    }

    #[tokio::test]
    async fn should_fail_for_anonymous_owner() {
        let setup = SetupBuilder::new().build().await;

        let result = setup
            .minter()
            .try_get_deposit_address(GetDepositAddressArgs {
                owner: Some(Principal::anonymous()),
                subaccount: None,
            })
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("the owner must be non-anonymous"));
    }

    #[tokio::test]
    async fn should_fail_for_anonymous_caller_and_no_owner() {
        let setup = SetupBuilder::new()
            .with_caller(Principal::anonymous())
            .build()
            .await;

        let result = setup
            .minter()
            .try_get_deposit_address(GetDepositAddressArgs {
                owner: None,
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

        assert_eq!(deposit_address.to_string(), DEFAULT_CALLER_DEPOSIT_ADDRESS);
    }
}
