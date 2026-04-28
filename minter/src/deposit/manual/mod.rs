use crate::{
    constants::GET_TRANSACTION_CYCLES,
    cycles::{charge_caller_cycles, check_caller_available_cycles},
    deposit::fetch_and_validate_deposit,
    guard::process_deposit_guard,
    ledger::mint,
    runtime::CanisterRuntime,
    state::{
        Deposit,
        audit::process_event,
        event::{DepositId, DepositSource, EventType},
        mutate_state, read_state,
    },
};
use canlog::log;
use cksol_types::{DepositStatus, ProcessDepositError};
use cksol_types_internal::log::Priority;
use icrc_ledger_types::icrc1::account::Account;
use solana_signature::Signature;

#[cfg(test)]
mod tests;

pub async fn process_deposit<R: CanisterRuntime>(
    runtime: R,
    account: Account,
    signature: Signature,
) -> Result<DepositStatus, ProcessDepositError> {
    let _guard = process_deposit_guard(account)?;

    let deposit_id = DepositId { account, signature };

    let Deposit {
        deposit_amount,
        amount_to_mint,
        source: _,
    } = match read_state(|state| state.deposit_status(&deposit_id)) {
        None => try_accept_deposit(&runtime, account, signature).await?,
        Some(DepositStatus::Processing {
            deposit_amount,
            amount_to_mint,
            deposit_id: _,
        }) => Deposit {
            deposit_amount,
            amount_to_mint,
            source: DepositSource::Manual,
        },
        // Deposit is already fully processed, nothing more to do
        Some(status @ (DepositStatus::Quarantined(_) | DepositStatus::Minted { .. })) => {
            return Ok(status);
        }
    };

    match mint(&runtime, deposit_id, amount_to_mint).await {
        Ok(deposit_status) => Ok(deposit_status),
        Err(e) => {
            log!(
                Priority::Info,
                "Error minting tokens for deposit {deposit_id:?}: {e}"
            );
            Ok(DepositStatus::Processing {
                deposit_amount,
                amount_to_mint,
                deposit_id: deposit_id.into(),
            })
        }
    }
}

async fn try_accept_deposit<R: CanisterRuntime>(
    runtime: &R,
    account: Account,
    signature: Signature,
) -> Result<Deposit, ProcessDepositError> {
    let (cycles_to_attach, deposit_consolidation_fee, fee) = read_state(|state| {
        (
            state.process_deposit_required_cycles(),
            state.deposit_consolidation_fee(),
            state.manual_deposit_fee(),
        )
    });
    check_caller_available_cycles(runtime, cycles_to_attach)?;

    let result = fetch_and_validate_deposit(runtime, account, signature, fee).await;

    // Always charge for the RPC call; additionally charge the consolidation fee if a deposit is found
    let rpc_cost = GET_TRANSACTION_CYCLES.saturating_sub(runtime.msg_cycles_refunded());
    let cycles_to_charge = rpc_cost
        + if result.is_ok() {
            deposit_consolidation_fee
        } else {
            0
        };
    charge_caller_cycles(runtime, cycles_to_charge);

    let (deposit_id, deposit_amount, amount_to_mint) = result?;

    mutate_state(|state| {
        process_event(
            state,
            EventType::AcceptedDeposit {
                deposit_id,
                deposit_amount,
                amount_to_mint,
                source: DepositSource::Manual,
            },
            runtime,
        )
    });
    log!(
        Priority::Info,
        "Accepted manual deposit {deposit_id:?}: {deposit_amount} lamports deposited, minting {amount_to_mint} lamports"
    );
    Ok(Deposit {
        deposit_amount,
        amount_to_mint,
        source: DepositSource::Manual,
    })
}
