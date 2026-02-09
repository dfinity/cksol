use candid::Principal;
use cksol_types::{Address, GetDepositAddressArgs};

mod address;

#[ic_cdk::update(name = "getDepositAddress")]
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
