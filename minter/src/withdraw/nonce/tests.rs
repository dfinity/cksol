use crate::{
    test_fixtures::{
        MINTER_ADDRESS, durable_nonce, init_state, nonce_account_address, nonce_account_info,
        runtime::TestCanisterRuntime,
    },
    withdraw::nonce::read_verified_nonce,
};
use solana_address::Address;

type GetAccountInfoResult = sol_rpc_types::MultiRpcResult<Option<sol_rpc_types::AccountInfo>>;

#[tokio::test]
async fn should_return_the_nonce_value_of_an_account_with_the_minter_as_authority() {
    init_state();

    let runtime = TestCanisterRuntime::new().add_stub_response(GetAccountInfoResult::Consistent(
        Ok(Some(nonce_account_info(MINTER_ADDRESS, 1))),
    ));

    let result = read_verified_nonce(&runtime, nonce_account_address(), MINTER_ADDRESS).await;

    assert_eq!(result, Ok(durable_nonce(1)));
}

#[tokio::test]
#[should_panic(expected = "BUG: nonce account")]
async fn should_panic_if_the_account_is_not_owned_by_the_system_program() {
    init_state();

    let foreign_owner_account = sol_rpc_types::AccountInfo {
        owner: MINTER_ADDRESS.to_string(),
        ..nonce_account_info(MINTER_ADDRESS, 1)
    };

    let runtime = TestCanisterRuntime::new().add_stub_response(GetAccountInfoResult::Consistent(
        Ok(Some(foreign_owner_account)),
    ));

    let _ = read_verified_nonce(&runtime, nonce_account_address(), MINTER_ADDRESS).await;
}

#[tokio::test]
#[should_panic(expected = "BUG: nonce account")]
async fn should_panic_if_the_authority_is_not_the_minter_address() {
    init_state();
    let other_authority = Address::from([0x99; 32]);

    let runtime = TestCanisterRuntime::new().add_stub_response(GetAccountInfoResult::Consistent(
        Ok(Some(nonce_account_info(other_authority, 1))),
    ));

    let _ = read_verified_nonce(&runtime, nonce_account_address(), MINTER_ADDRESS).await;
}
