use crate::{
    address::{lazy_get_schnorr_master_key, minter_account, minter_address},
    runtime::CanisterRuntime,
    state::{audit::process_event, event::EventType, mutate_state, read_state},
    transaction::get_balance,
};
use canlog::log;
use cksol_types_internal::log::Priority;
use std::time::Duration;

pub async fn lazy_init_consolidated_balance<R: CanisterRuntime + 'static>(runtime: R) {
    if read_state(|s| s.consolidated_balance().is_some()) {
        return;
    }

    let master_key = lazy_get_schnorr_master_key().await;
    let address = minter_address(&master_key, &runtime);
    let account = minter_account(&runtime);

    match get_balance(&runtime, address).await {
        Ok(balance) => {
            mutate_state(|s| {
                process_event(
                    s,
                    EventType::SyncedAccountBalance { account, balance },
                    &runtime,
                )
            });
        }
        Err(e) => {
            log!(
                Priority::Error,
                "Failed to fetch initial minter balance: {e}"
            );
            ic_cdk_timers::set_timer(Duration::from_secs(60), async {
                lazy_init_consolidated_balance(runtime).await;
            });
        }
    }
}
