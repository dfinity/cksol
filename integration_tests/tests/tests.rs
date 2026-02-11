mod get_deposit_address_tests {
    use candid::Principal;
    use cksol_int_tests::{Setup, SetupBuilder};
    use cksol_types::GetDepositAddressArgs;
    use icrc_ledger_types::icrc1::account::Subaccount;

    async fn get_deposit_address(
        setup: &Setup,
        owner: Option<Principal>,
        subaccount: Option<Subaccount>,
    ) -> String {
        setup
            .minter()
            .get_deposit_address(GetDepositAddressArgs { owner, subaccount })
            .await
            .to_string()
    }

    #[tokio::test]
    async fn should_get_deposit_address() {
        let setup = SetupBuilder::new().build().await;

        const DEFAULT_CALLER_DEPOSIT_ADDRESS: &str = "Ge2aoiaTb6Tq2DQ4xs7qGhGud97pKtDmJCAQufTJeNSu";

        // Owner is the default caller
        assert_eq!(
            get_deposit_address(&setup, None, None).await,
            DEFAULT_CALLER_DEPOSIT_ADDRESS
        );

        // Different owner
        assert_eq!(
            get_deposit_address(&setup, Some(Principal::from_slice(&[1])), None).await,
            "9qvNPGSFQY8fvmr5A2jyCmSBfN7rrWBGJEAGgpN2TKeV"
        );

        // Owner is the default caller, but different subaccounts specified
        assert_eq!(
            get_deposit_address(&setup, None, Some([1; 32])).await,
            "97eLNQ1sc7yQHscLWet7vq7AZ6TbxN5nx8D8LPSbYEJB"
        );
        assert_eq!(
            get_deposit_address(&setup, None, Some([2; 32])).await,
            "BiuUj1yMbtStuumWutpBajSjNDPbnE5dNEuTv7J1cjmB"
        );

        // Caller is anonymous, but we specify the owner explicitly
        let setup = SetupBuilder::new()
            .with_caller(Principal::anonymous())
            .build()
            .await;

        assert_eq!(
            get_deposit_address(&setup, Some(Setup::DEFAULT_CALLER), None).await,
            DEFAULT_CALLER_DEPOSIT_ADDRESS
        );
    }

    #[tokio::test]
    async fn should_fail_for_anonymous_owner() {
        let setup = SetupBuilder::new().build().await;

        // Caller is default caller, but the owner is specified explicitly to anonymous
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

        // Anonymous caller and owner not specified
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
}
