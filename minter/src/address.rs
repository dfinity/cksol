use candid::Principal;
use ic_cdk::management_canister::{SchnorrAlgorithm, SchnorrKeyId, SchnorrPublicKeyArgs};
use icrc_ledger_types::icrc1::account::Subaccount;
use solana_address::Address;

pub async fn get_deposit_address(principal: Principal, subaccount: Option<Subaccount>) -> Address {
    let args = SchnorrPublicKeyArgs {
        canister_id: None,
        derivation_path: vec![
            principal.as_slice().to_vec(),
            subaccount.unwrap_or_default().to_vec(),
        ],
        key_id: SchnorrKeyId {
            algorithm: SchnorrAlgorithm::Ed25519,
            name: "dfx_test_key".to_string(),
        },
    };
    let result = ic_cdk::management_canister::schnorr_public_key(&args)
        .await
        .unwrap();
    Address::try_from(result.public_key.as_slice()).unwrap_or_else(|_| {
        panic!(
            "Expected Schnorr public key to be 32 bytes, but got: {} bytes",
            result.public_key.len()
        )
    })
}
