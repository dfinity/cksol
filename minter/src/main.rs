use candid::Principal;
use cksol_types::{Address, GetDepositAddressArgs, UpdateBalanceArgs, UpdateBalanceError};
use sol_rpc_types::Lamport;
use transaction::try_get_transaction;

mod address;
mod state;
mod transaction;

#[ic_cdk::update]
async fn get_deposit_address(args: GetDepositAddressArgs) -> Address {
    let owner = args.owner.unwrap_or_else(ic_cdk::api::msg_caller);
    assert_ne!(
        owner,
        Principal::anonymous(),
        "the owner must be non-anonymous"
    );

    address::get_deposit_address(owner, args.subaccount)
        .await
        .into()
}

#[ic_cdk::update(hidden = true)]
async fn update_balance(args: UpdateBalanceArgs) -> Result<Option<Lamport>, UpdateBalanceError> {
    let owner = args.owner.unwrap_or_else(ic_cdk::api::msg_caller);
    assert_ne!(
        owner,
        Principal::anonymous(),
        "the owner must be non-anonymous"
    );

    let deposit_address = address::get_deposit_address(owner, args.subaccount).await;

    let maybe_transaction = try_get_transaction(args.signature)
        .await
        .map_err(UpdateBalanceError::GetTransactionError);
    let transaction = maybe_transaction?.ok_or(UpdateBalanceError::TransactionNotFound)?;

    let maybe_deposit = transaction::get_deposit_amount_to_address(transaction, deposit_address)
        .map_err(UpdateBalanceError::InvalidTransaction)?;

    Ok(maybe_deposit)
}

fn main() {}

#[test]
fn check_candid_interface_compatibility() {
    use candid_parser::utils::{CandidSource, service_equal};

    candid::export_service!();

    let new_interface = __export_service();

    // check the public interface against the actual one
    let old_interface = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap())
        .join("cksol-minter.did");

    service_equal(
        CandidSource::Text(dbg!(&new_interface)),
        CandidSource::File(old_interface.as_path()),
    )
    .unwrap();
}
