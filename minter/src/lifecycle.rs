use crate::state::mutate_state;
use canlog::log;
use cksol_types_internal::{InitArgs, UpgradeArgs, log::Priority};

pub fn init(args: InitArgs) {
    log!(
        Priority::Info,
        "[init]: initialized ckSOL minter with args: {:?}",
        args
    );
    let InitArgs {
        sol_rpc_canister_id,
        ledger_canister_id,
        deposit_fee,
        master_key_name,
    } = args;
    mutate_state(|s| {
        s.sol_rpc_canister_id = sol_rpc_canister_id;
        s.ledger_canister_id = ledger_canister_id;
        s.master_key_name = master_key_name;
        s.deposit_fee = deposit_fee;
    });
}

pub fn post_upgrade(args: Option<UpgradeArgs>) {
    if let Some(UpgradeArgs {
        deposit_fee,
        ledger_canister_id,
        sol_rpc_canister_id,
    }) = args
    {
        if let Some(deposit_fee) = deposit_fee {
            mutate_state(|s| s.deposit_fee = deposit_fee);
        }
        if let Some(sol_rpc_canister_id) = sol_rpc_canister_id {
            mutate_state(|s| s.sol_rpc_canister_id = sol_rpc_canister_id);
        }
        if let Some(ledger_canister_id) = ledger_canister_id {
            mutate_state(|s| s.ledger_canister_id = ledger_canister_id);
        }
    }
}
