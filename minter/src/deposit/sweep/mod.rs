use crate::{
    address::{account_address, lazy_get_schnorr_master_key},
    constants::{GET_BALANCE_CYCLES, RENT_EXEMPTION_THRESHOLD},
    cycles::{RpcCallCharge, charge_rpc_call, check_caller_available_cycles},
    guard::deposit_sol_guard,
    rpc::get_balance,
    runtime::CanisterRuntime,
    state::{audit::process_event, event::EventType, mutate_state, read_state},
    utils::assert_non_anonymous_account,
};
use canlog::log;
use cksol_types::{DepositSolError, DepositSolId, DepositSolStatus, Lamport};
use cksol_types_internal::log::Priority;
use icrc_ledger_types::icrc1::account::Account;

#[cfg(test)]
mod tests;

mod timer;

pub use crate::constants::SWEEP_DEPOSITS_DELAY;
pub use timer::sweep_queued_deposits;

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
        return Ok(deposit_id);
    }

    // TODO hq-3k1.6: This check only exists while `process_deposit` still mints before the
    // deposit is consolidated, and will be removed together with that endpoint by the last PR
    // of the stack. Without it, a `deposit_sol` call while a `process_deposit` call is running
    // or its deposit is accepted, minted or awaiting consolidation would sweep lamports that
    // `process_deposit` credits or has credited, and mint them twice.
    if read_state(|state| state.has_process_deposit_in_progress(&account)) {
        return Err(DepositSolError::TemporarilyUnavailable(
            "a process_deposit call or deposit for this account is in progress or awaiting \
             consolidation, try again once it has been consolidated"
                .to_string(),
        ));
    }

    let master_key = lazy_get_schnorr_master_key(runtime).await;
    let deposit_address = account_address(&master_key, &account);
    let result = get_balance(runtime, deposit_address)
        .await
        .map_err(DepositSolError::from)
        .and_then(|balance| sweepable_amount_above_minimum(balance, minimum_deposit_amount));
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
        let deposit_id = state.next_deposit_sol_id();
        process_event(
            state,
            EventType::QueuedDeposit {
                deposit_id,
                account,
                sweepable_amount,
            },
            runtime,
        );
        deposit_id
    });
    log!(
        Priority::Info,
        "Queued deposit {deposit_id} for account {account:?}: {sweepable_amount} lamports sweepable from {deposit_address}"
    );
    Ok(deposit_id)
}

pub fn deposit_status(deposit_id: DepositSolId) -> DepositSolStatus {
    read_state(|state| state.deposit_sol_status(deposit_id))
}

fn sweepable_amount_above_minimum(
    balance: Lamport,
    minimum_deposit_amount: Lamport,
) -> Result<Lamport, DepositSolError> {
    if balance < minimum_deposit_amount {
        return Err(DepositSolError::ValueTooSmall {
            balance,
            minimum_deposit_amount,
        });
    }
    Ok(sweepable_amount(balance))
}

fn sweepable_amount(balance: Lamport) -> Lamport {
    balance.saturating_sub(RENT_EXEMPTION_THRESHOLD)
}
