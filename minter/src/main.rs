use cksol_minter::get_sol_address;
use cksol_types::{DummyRequest, DummyResponse, GetSolAddressArgs};

#[ic_cdk::query]
fn greet(request: DummyRequest) -> DummyResponse {
    DummyResponse {
        output: format!("Hello, {}!", request.input),
    }
}

#[ic_cdk::query]
async fn get_sol_address(request: GetSolAddressArgs) -> String {
    cksol_minter::get_sol_address(request).await
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
