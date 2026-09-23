use crate::{
    address::{account_address, lazy_get_schnorr_master_key},
    constants::{GET_BALANCE_CYCLES, RENT_EXEMPTION_THRESHOLD},
    cycles::{RpcCallCharge, charge_rpc_call, check_caller_available_cycles},
    guard::deposit_sol_guard,
    rpc::get_balance,
    runtime::CanisterRuntime,
    state::{QueuedDeposit, audit::queue_deposit, mutate_state, read_state},
    utils::assert_non_anonymous_account,
};
use canlog::log;
use cksol_types::{DepositSolError, DepositSolId, DepositSolStatus, Lamport};
use cksol_types_internal::log::Priority;
use icrc_ledger_types::icrc1::account::Account;

#[cfg(test)]
mod tests;

pub async fn deposit_sol<R: CanisterRuntime>(
    runtime: &R,
    account: Account,
) -> Result<DepositSolId, DepositSolError> {
    assert_non_anonymous_account(&account);
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

    if let Some(deposit_id) = read_state(|state| state.in_flight_deposit_id(&account)) {
        return Err(DepositSolError::DepositInFlight { deposit_id });
    }

    let master_key = lazy_get_schnorr_master_key(runtime).await;
    let deposit_address = account_address(&master_key, &account);
    let result = get_balance(runtime, deposit_address)
        .await
        .map_err(DepositSolError::from)
        .and_then(|balance| check_sweepable_amount(balance, minimum_deposit_amount));
    charge_rpc_call(
        runtime,
        RpcCallCharge {
            attached_cycles: GET_BALANCE_CYCLES,
            fee_on_success: deposit_consolidation_fee,
        },
        &result,
    );
    let sweepable_amount = result?;

    let deposit_id = mutate_state(|state| {
        queue_deposit(
            state,
            QueuedDeposit {
                account,
                sweepable_amount,
            },
            runtime,
        )
    });
    log!(
        Priority::Info,
        "Queued deposit {deposit_id} for account {account:?}: {sweepable_amount} lamports sweepable from {deposit_address}"
    );
    Ok(deposit_id)
}

pub fn deposit_status(deposit_id: DepositSolId) -> Option<DepositSolStatus> {
    read_state(|state| state.deposit_sol_status(deposit_id))
}

fn check_sweepable_amount(
    balance: Lamport,
    minimum_deposit_amount: Lamport,
) -> Result<Lamport, DepositSolError> {
    let sweepable_amount = sweepable_amount(balance);
    if sweepable_amount < minimum_deposit_amount {
        return Err(DepositSolError::ValueTooSmall {
            sweepable_amount,
            minimum_deposit_amount,
        });
    }
    Ok(sweepable_amount)
}

fn sweepable_amount(balance: Lamport) -> Lamport {
    balance.saturating_sub(RENT_EXEMPTION_THRESHOLD)
}
