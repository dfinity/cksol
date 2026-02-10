use crate::logs::Priority;
use candid::Principal;
use canlog::log;
use cksol_types::{Address, GetDepositAddressArgs, UpdateBalanceArgs, UpdateBalanceError};
use icrc_ledger_types::icrc1::account::{Account, Subaccount};
use sol_rpc_types::Lamport;
use transaction::try_get_transaction;

mod address;
mod logs;
mod state;
mod transaction;

#[ic_cdk::update]
async fn get_deposit_address(args: GetDepositAddressArgs) -> Address {
    let account = assert_non_anonymous_account(args.owner, args.subaccount);
    address::get_deposit_address(account).await.into()
}

#[ic_cdk::update]
async fn update_balance(args: UpdateBalanceArgs) -> Result<Lamport, UpdateBalanceError> {
    let account = assert_non_anonymous_account(args.owner, args.subaccount);
    let deposit_address = address::get_deposit_address(account).await;

    let maybe_transaction = try_get_transaction(args.signature).await.map_err(|e| {
        log!(Priority::Debug, "Error while fetching transaction: {e}");
        UpdateBalanceError::TransientRpcError
    });
    let transaction = maybe_transaction?.ok_or(UpdateBalanceError::TransactionNotFound)?;

    let deposit = transaction::get_deposit_amount_to_address(transaction, deposit_address)
        .map_err(UpdateBalanceError::InvalidDepositTransaction)?;

    Ok(deposit)
}

fn assert_non_anonymous_account(
    owner: Option<Principal>,
    subaccount: Option<Subaccount>,
) -> Account {
    let owner = owner.unwrap_or_else(ic_cdk::api::msg_caller);
    assert_ne!(
        owner,
        Principal::anonymous(),
        "the owner must be non-anonymous"
    );
    Account { owner, subaccount }
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
