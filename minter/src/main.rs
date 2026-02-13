use crate::logs::Priority;
use crate::state::{State, mutate_state, read_state};
use candid::Principal;
use canlog::log;
use cksol_types::{
    Address, DepositStatus, GetDepositAddressArgs, InstallArgs, UpdateBalanceArgs,
    UpdateBalanceError,
};
use ic_canister_runtime::IcRuntime;
use icrc_ledger_types::icrc1::account::{Account, Subaccount};
use transaction::try_get_transaction;

mod address;
mod ledger;
mod logs;
mod state;
mod transaction;

#[ic_cdk::init]
async fn init(args: InstallArgs) {
    mutate_state(|state| {
        *state = State {
            master_public_key: None,
            master_key_name: args.master_key_name,
            sol_rpc_canister_id: args.sol_rpc_canister_id,
            ledger_canister_id: args.ledger_canister_id,
            deposit_fee: args.deposit_fee,
            log_filter: args.log_filter.unwrap_or_default(),
        }
    })
}

#[ic_cdk::update]
async fn get_deposit_address(args: GetDepositAddressArgs) -> Address {
    let account = assert_non_anonymous_account(args.owner, args.subaccount);
    address::get_deposit_address(account).await.into()
}

#[ic_cdk::update]
async fn update_balance(args: UpdateBalanceArgs) -> Result<DepositStatus, UpdateBalanceError> {
    let account = assert_non_anonymous_account(args.owner, args.subaccount);
    let deposit_address = address::get_deposit_address(account).await;
    let signature = args.signature;

    let transaction = try_get_transaction(IcRuntime::new(), signature.clone().into())
        .await
        .map_err(|e| {
            log!(
                Priority::Info,
                "Error fetching transaction with signature {signature}: {e}"
            );
            e
        })?;

    let deposit_amount = transaction::get_deposit_amount_to_address(transaction, deposit_address)
        .map_err(|e| {
        log!(
            Priority::Info,
            "Error parsing deposit transaction with signature {signature}: {e}"
        );
        UpdateBalanceError::InvalidDepositTransaction(e.to_string())
    })?;

    let deposit_fee = read_state(|state| state.deposit_fee);
    if deposit_amount < deposit_fee {
        return Ok(DepositStatus::ValueTooSmall(signature));
    }
    let mint_amount = deposit_amount - deposit_fee;

    ledger::mint(IcRuntime::new(), account, mint_amount, signature).await
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
