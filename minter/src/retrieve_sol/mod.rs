use candid::Nat;
use cksol_types::{Address, RetrieveSolError, RetrieveSolOk};
use icrc_ledger_client_cdk::{CdkRuntime, ICRC1Client};
use icrc_ledger_types::{icrc1::account::Account, icrc2::transfer_from::TransferFromArgs};

use crate::state::read_state;

pub async fn retrieve_sol(
    from: Account,
    amount: u64,
    _to: Address,
) -> Result<RetrieveSolOk, RetrieveSolError> {
    let ledger_canister_id = read_state(|s| s.ledger_canister_id());
    let ledger_client = ICRC1Client {
        runtime: CdkRuntime,
        ledger_canister_id,
    };

    let args = TransferFromArgs {
        spender_subaccount: None,
        from,
        to: ic_cdk::api::canister_self().into(),
        amount: Nat::from(amount),
        fee: None,
        memo: None,
        created_at_time: None,
    };

    match ledger_client.transfer_from(args).await {
        Ok(result) => match result {
            Ok(_block_index) => todo!(),
            Err(transfer_error) => match transfer_error {
                _ => todo!(),
            },
        },
        Err(_) => Err(RetrieveSolError::InsufficientFunds { balance: 0 }),
    }
}
