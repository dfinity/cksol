use candid::Principal;
use cksol_minter::address;
use cksol_types::{
    Address, DepositStatus, GetDepositAddressArgs, UpdateBalanceArgs, UpdateBalanceError,
};
use cksol_types_internal::MinterArg;
use icrc_ledger_types::icrc1::account::{Account, Subaccount};

#[ic_cdk::init]
fn init(args: MinterArg) {
    match args {
        MinterArg::Init(init) => {
            cksol_minter::lifecycle::init(init);
        }
        MinterArg::Upgrade(_) => {
            ic_cdk::trap("cannot init canister state with upgrade args");
        }
    }
}

#[ic_cdk::post_upgrade]
fn post_upgrade(args: Option<MinterArg>) {
    match args {
        Some(MinterArg::Init(_)) => {
            ic_cdk::trap("cannot upgrade canister state with init args");
        }
        Some(MinterArg::Upgrade(args)) => {
            cksol_minter::lifecycle::post_upgrade(Some(args));
        }
        None => {
            cksol_minter::lifecycle::post_upgrade(None);
        }
    }
}

#[ic_cdk::update]
async fn get_deposit_address(args: GetDepositAddressArgs) -> Address {
    let account = assert_non_anonymous_account(args.owner, args.subaccount);
    address::get_deposit_address(account).await.into()
}

#[ic_cdk::update]
async fn update_balance(args: UpdateBalanceArgs) -> Result<DepositStatus, UpdateBalanceError> {
    let _account = assert_non_anonymous_account(args.owner, args.subaccount);
    Err(UpdateBalanceError::TemporarilyUnavailable(
        "Not yet implemented!".to_string(),
    ))
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
    use candid_parser::utils::{service_equal, CandidSource};

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
