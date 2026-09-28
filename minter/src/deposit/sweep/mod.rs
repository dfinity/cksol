use crate::{
    address::{account_address, lazy_get_schnorr_master_key},
    constants::GET_BALANCE_CYCLES,
    cycles::{RpcCallCharge, charge_rpc_call, check_caller_available_cycles},
    guard::deposit_sol_guard,
    rpc::get_balance,
    runtime::CanisterRuntime,
    state::{DepositBalance, audit::process_event, event::EventType, mutate_state, read_state},
    utils::assert_valid_deposit_owner,
};
use canlog::log;
use cksol_types::{DepositSolError, DepositSolId, DepositSolStatus, Lamport};
use cksol_types_internal::log::Priority;
use icrc_ledger_types::icrc1::account::Account;

#[cfg(test)]
mod tests;

mod finalize;
mod mint;
mod timer;

pub use crate::constants::{PROCESS_PENDING_MINTS_DELAY, SWEEP_DEPOSITS_DELAY};
pub use finalize::credit_finalized_sweeps;
pub use mint::process_pending_mints;
pub use timer::sweep_queued_deposits;

pub async fn deposit_sol<R: CanisterRuntime>(
    runtime: &R,
    account: Account,
) -> Result<DepositSolId, DepositSolError> {
    assert_valid_deposit_owner(&account, runtime.canister_self());
    let _guard = deposit_sol_guard(account)?;

    let (required_cycles, deposit_consolidation_fee, minimum_deposit_amount) =
        read_state(|state| {
            (
                state.process_deposit_required_cycles(),
                state.deposit_consolidation_fee(),
                state.minimum_deposit_amount(),
            )
        });
    check_caller_available_cycles(runtime, required_cycles)?;

    if let Some((deposit_id, status)) = read_state(|state| {
        state
            .deposits()
            .in_flight_id(&account)
            .map(|deposit_id| (deposit_id, state.deposits().status(deposit_id)))
    }) {
        return match status {
            DepositSolStatus::Queued { .. }
            | DepositSolStatus::Swept { .. }
            | DepositSolStatus::Finalized { .. } => Ok(deposit_id),
            DepositSolStatus::Quarantined { .. } => {
                Err(DepositSolError::Quarantined { deposit_id })
            }
            DepositSolStatus::Dropped { .. }
            | DepositSolStatus::Minted { .. }
            | DepositSolStatus::NotFound => panic!(
                "BUG: in-flight deposit {deposit_id} of account {account:?} has status {status:?}"
            ),
        };
    }

    let master_key = lazy_get_schnorr_master_key(runtime).await;
    let deposit_address = account_address(&master_key, &account);
    let result = get_balance(runtime, deposit_address)
        .await
        .map_err(DepositSolError::from)
        .and_then(|balance| balance_above_minimum(balance, minimum_deposit_amount));
    charge_rpc_call(
        runtime,
        RpcCallCharge {
            attached_cycles: GET_BALANCE_CYCLES,
            fee_on_success: deposit_consolidation_fee,
        },
        &result,
    );
    let balance = DepositBalance::new(result?)
        .expect("BUG: the minimum deposit amount covers the rent exemption threshold");

    let deposit_id = mutate_state(|state| {
        let deposit_id = state.deposits().next_id();
        process_event(
            state,
            EventType::QueuedDeposit {
                deposit_id,
                account,
                address: deposit_address,
                balance,
            },
            runtime,
        );
        deposit_id
    });
    log!(
        Priority::Info,
        "Queued deposit {deposit_id} for account {account:?}: {} lamports sweepable from {deposit_address}",
        balance.sweepable_amount()
    );
    Ok(deposit_id)
}

pub fn deposit_status(deposit_id: DepositSolId) -> DepositSolStatus {
    read_state(|state| state.deposits().status(deposit_id))
}

fn balance_above_minimum(
    balance: Lamport,
    minimum_deposit_amount: Lamport,
) -> Result<Lamport, DepositSolError> {
    if balance < minimum_deposit_amount {
        return Err(DepositSolError::ValueTooSmall {
            balance,
            minimum_deposit_amount,
        });
    }
    Ok(balance)
}
