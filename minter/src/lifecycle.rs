use crate::logs::Priority;
use crate::state::mutate_state;
use canlog::log;
use cksol_types_internal::{InitArgs, UpgradeArgs};

pub fn init(args: InitArgs) {
    log!(
        Priority::Info,
        "[init]: initialized ckSOL minter with args: {:?}",
        args
    );
    let InitArgs {
        sol_rpc_canister_id: _,
        ledger_canister_id: _,
        deposit_fee,
        master_key_name,
    } = args;
    mutate_state(|s| {
        s.master_key_name = master_key_name;
        s.deposit_fee = deposit_fee;
    });
}

pub fn post_upgrade(args: Option<UpgradeArgs>) {
    if let Some(UpgradeArgs { deposit_fee }) = args
        && let Some(deposit_fee) = deposit_fee
    {
        mutate_state(|s| s.deposit_fee = deposit_fee);
    }
}
