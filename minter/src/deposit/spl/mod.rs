use crate::{
    address::{account_address, associated_token_address, minter_public_key},
    constants::GET_ACCOUNT_INFO_CYCLES,
    cycles::{RpcCallCharge, charge_caller_cycles, charge_rpc_call, check_caller_available_cycles},
    guard::deposit_spl_guard,
    rpc::get_spl_token_balance,
    runtime::CanisterRuntime,
    state::{
        InFlightSplDeposit, QueuedSplDeposit, SupportedSplToken, audit::process_event,
        event::EventType, mutate_state, read_state,
    },
    utils::{assert_valid_deposit_owner, assert_valid_deposit_token},
};
use canlog::log;
use cksol_types::{DepositSplError, DepositSplId, DepositSplStatus};
use cksol_types_internal::log::Priority;
use icrc_ledger_types::icrc1::account::Account;
use solana_address::Address;

#[cfg(test)]
mod tests;

mod failure;
pub(crate) use failure::failed_withdrawal_reason;

mod finalize;
pub use finalize::{credit_finalized_spl_sweeps, settle_failed_spl_sweeps};

mod mint;
pub use mint::process_pending_spl_mints;

mod timer;
#[cfg(any(test, feature = "canbench-rs"))]
pub(crate) use timer::reset_cursor;
pub use timer::{SWEEP_SPL_DEPOSITS_DELAY, sweep_queued_spl_deposits};

/// Accepts one manual classic-SPL deposit. No treasury read, scan, sweep or mint is performed.
pub async fn deposit_spl<R: CanisterRuntime>(
    runtime: &R,
    account: Account,
    mint: cksol_types::Address,
) -> Result<DepositSplId, DepositSplError> {
    assert_valid_deposit_owner(&account, runtime.canister_self());
    let mint = Address::from(mint);
    let _guard = deposit_spl_guard(account, mint)?;
    // A pause stops new deposits only, so an in-flight deposit stays reachable.
    match read_state(|state| state.spl_deposits().in_flight(&account, &mint)) {
        Some(InFlightSplDeposit::Processing(deposit_id)) => return Ok(deposit_id),
        Some(InFlightSplDeposit::Quarantined(deposit_id)) => {
            return Err(DepositSplError::Quarantined { deposit_id });
        }
        None => {}
    }
    assert_valid_deposit_token(&mint)?;
    let consolidation_fee = check_deposit_cycles(runtime)?;
    let master_key = minter_public_key()
        .map_err(|error| DepositSplError::TemporarilyUnavailable(error.to_string()))?;
    let owner = account_address(&master_key, &account);
    let address = associated_token_address(&owner, &mint);
    let latest_deposit_before_read = read_state(|s| s.spl_deposits().latest_id(&account, &mint));
    let result = read_deposit_balance(runtime, address, owner, mint).await;
    if let Some(id) = deposit_queued_meanwhile(&account, &mint, latest_deposit_before_read) {
        return Ok(id);
    }
    let balance = result?;
    let token = validate_deposit_balance(&mint, balance)?;
    charge_caller_cycles(runtime, consolidation_fee);
    Ok(queue_deposit(
        runtime,
        QueuedSplDeposit {
            account,
            mint,
            address,
            balance,
            fee: token.deposit_fee,
        },
    ))
}

/// Checks the upfront cycle budget and captures the consolidation fee before the read.
fn check_deposit_cycles<R: CanisterRuntime>(runtime: &R) -> Result<u128, DepositSplError> {
    let (required_cycles, consolidation_fee) = read_state(|state| {
        (
            state.deposit_spl_required_cycles(),
            state.spl_deposit_consolidation_fee(),
        )
    });
    check_caller_available_cycles(runtime, required_cycles)?;
    Ok(consolidation_fee)
}

/// Charges the actual source RPC cost, including when the read or decoding fails.
async fn read_deposit_balance<R: CanisterRuntime>(
    runtime: &R,
    address: Address,
    owner: Address,
    mint: Address,
) -> Result<u64, DepositSplError> {
    let result = get_spl_token_balance(runtime, address, owner, mint)
        .await
        .map_err(DepositSplError::from);
    charge_rpc_call(
        runtime,
        RpcCallCharge {
            attached_cycles: GET_ACCOUNT_INFO_CYCLES,
            fee_on_success: 0,
        },
        &result,
    );
    result
}

/// The deposit queued for the pair while the balance read was in flight.
/// The read must not queue a second deposit from the same balance.
fn deposit_queued_meanwhile(
    account: &Account,
    mint: &Address,
    latest_deposit_before_read: Option<DepositSplId>,
) -> Option<DepositSplId> {
    let latest = read_state(|state| state.spl_deposits().latest_id(account, mint));
    (latest != latest_deposit_before_read)
        .then_some(latest)
        .flatten()
}

/// Rechecks current registration, pause and deposit terms after the RPC await.
fn validate_deposit_balance(
    mint: &Address,
    balance: u64,
) -> Result<SupportedSplToken, DepositSplError> {
    let token = assert_valid_deposit_token(mint)?;
    let minimum = token
        .minimum_deposit_amount
        .checked_add(token.deposit_fee)
        .expect("BUG: registered SPL deposit minimum plus fee overflows");
    if balance < minimum {
        return Err(DepositSplError::ValueTooSmall {
            balance,
            minimum_balance: minimum,
        });
    }
    Ok(token)
}

pub fn deposit_spl_status(deposit_id: DepositSplId) -> DepositSplStatus {
    read_state(|state| state.spl_deposits().status(deposit_id))
}

/// Queues the deposit under the next deposit id and returns that id.
///
/// # Panics
///
/// If the account and mint pair of the deposit already has a deposit in flight, which
/// callers must rule out after their balance read.
fn queue_deposit<R: CanisterRuntime>(runtime: &R, deposit: QueuedSplDeposit) -> DepositSplId {
    let deposit_id = mutate_state(|state| {
        let deposit_id = state.spl_deposits().next_id();
        process_event(
            state,
            EventType::QueuedSplDeposit {
                deposit_id,
                account: deposit.account,
                mint: deposit.mint,
                address: deposit.address,
                balance: deposit.balance,
                fee: deposit.fee,
            },
            runtime,
        );
        deposit_id
    });
    log!(
        Priority::Info,
        "Queued SPL deposit {deposit_id} for account {:?}: {} units of mint {} sweepable from {}, with a fee of {} units",
        deposit.account,
        deposit.balance,
        deposit.mint,
        deposit.address,
        deposit.fee
    );
    deposit_id
}
