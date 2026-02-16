use std::str::FromStr;

use candid::Principal;
use cksol_types::{
    Address, GetDepositAddressArgs, RetrieveSolArgs, RetrieveSolError, RetrieveSolOk,
    RetrieveSolStatus,
};
use cksol_types_internal::MinterArg;

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
    let owner = args.owner.unwrap_or_else(ic_cdk::api::msg_caller);
    assert_ne!(
        owner,
        Principal::anonymous(),
        "the owner must be non-anonymous"
    );

    cksol_minter::address::get_deposit_address(owner, args.subaccount)
        .await
        .into()
}

#[ic_cdk::update]
async fn retrieve_sol(args: RetrieveSolArgs) -> Result<RetrieveSolOk, RetrieveSolError> {
    let _solana_address = Address::from_str(&args.address)
        .map_err(|e| return RetrieveSolError::MalformedAddress(e.to_string()))?;
    Err(RetrieveSolError::InsufficientFunds { balance: 0 })
}

#[ic_cdk::update]
async fn retrieve_sol_status(_block_index: u64) -> RetrieveSolStatus {
    RetrieveSolStatus::NotFound
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
