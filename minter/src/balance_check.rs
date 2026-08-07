use crate::{
    address::{lazy_get_schnorr_master_key, minter_address},
    guard::TimerGuard,
    rpc::get_balance,
    runtime::CanisterRuntime,
    state::{TaskType, mutate_state},
};
use canlog::log;
use cksol_types_internal::log::Priority;
use std::time::Duration;

pub const REFRESH_REAL_BALANCE_DELAY: Duration = Duration::from_secs(24 * 60 * 60);

/// Refresh the cached on-chain balance of the minter's main account.
/// Each call makes an RPC request to the Solana network, so this runs at most
/// once per day (see [`REFRESH_REAL_BALANCE_DELAY`]).
pub async fn refresh_real_balance<R: CanisterRuntime>(runtime: R) {
    let _guard = match TimerGuard::new(TaskType::RefreshRealBalance) {
        Ok(guard) => guard,
        Err(_) => return,
    };

    let master_key = lazy_get_schnorr_master_key(&runtime).await;
    let address = minter_address(&master_key, &runtime);

    match get_balance(&runtime, address).await {
        Ok(balance) => {
            let observed_at = runtime.time();
            let diff = mutate_state(|s| {
                s.record_balance_discrepancy(balance, observed_at);
                s.last_balance_discrepancy()
                    .expect("BUG: just-recorded discrepancy must be present")
                    .diff_lamports
            });
            log!(
                Priority::Info,
                "Refreshed real balance for minter {address}: {balance} lamports (discrepancy vs. tracked: {diff} lamports)"
            );
        }
        Err(e) => {
            log!(
                Priority::Info,
                "Failed to refresh real balance for minter {address}: {e}"
            );
        }
    }
}
