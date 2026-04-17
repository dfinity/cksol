use crate::{
    address::get_deposit_address,
    cycles::{charge_caller_cycles, check_caller_available_cycles},
    deposit::get_deposit_amount_to_address,
    guard::update_balance_guard,
    ledger::mint,
    rpc::get_transaction,
    runtime::CanisterRuntime,
    state::{
        Deposit,
        audit::process_event,
        event::{DepositId, EventType},
        mutate_state, read_state,
    },
};
use canlog::log;
use cksol_types::{DepositStatus, UpdateBalanceError};
use cksol_types_internal::log::Priority;
use icrc_ledger_types::icrc1::account::Account;
use solana_signature::Signature;

#[cfg(test)]
mod tests;

pub async fn update_balance<R: CanisterRuntime>(
    runtime: R,
    account: Account,
    signature: Signature,
) -> Result<DepositStatus, UpdateBalanceError> {
    let _guard = update_balance_guard(account)?;

    let deposit_id = DepositId { account, signature };

    let Deposit {
        deposit_amount,
        amount_to_mint,
    } = match read_state(|state| state.deposit_status(&deposit_id)) {
        None => try_accept_deposit(&runtime, account, signature, deposit_id).await?,
        Some(DepositStatus::Processing {
            deposit_amount,
            amount_to_mint,
            deposit_id: _,
        }) => Deposit {
            deposit_amount,
            amount_to_mint,
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
    deposit_id: DepositId,
) -> Result<Deposit, UpdateBalanceError> {
    let (cycles_to_attach, deposit_consolidation_fee) = read_state(|state| {
        (
            state.update_balance_required_cycles(),
            state.deposit_consolidation_fee(),
        )
    });
    check_caller_available_cycles(runtime, cycles_to_attach)?;

    // Reserve the consolidation fee and forward the rest to the HTTP outcall
    let cycles_for_rpc = cycles_to_attach.saturating_sub(deposit_consolidation_fee);
    let maybe_transaction = get_transaction(runtime, signature, cycles_for_rpc)
        .await
        .map_err(|e| {
            log!(
                Priority::Info,
                "Error fetching transaction for deposit {deposit_id:?}: {e}"
            );
            UpdateBalanceError::from(e)
        })?;

    // Charge the actual RPC cost plus the consolidation fee
    let rpc_cost = cycles_for_rpc.saturating_sub(runtime.msg_cycles_refunded());
    charge_caller_cycles(runtime, rpc_cost + deposit_consolidation_fee);

    let transaction = match maybe_transaction {
        Some(transaction) => Ok(transaction),
        None => Err(UpdateBalanceError::TransactionNotFound),
    }?;

    let deposit_address = get_deposit_address(account).await;
    let deposit_amount =
        get_deposit_amount_to_address(transaction, deposit_address).map_err(|e| {
            log!(
                Priority::Info,
                "Error parsing deposit transaction with signature {signature}: {e}"
            );
            UpdateBalanceError::InvalidDepositTransaction(e.to_string())
        })?;
    let minimum_deposit_amount = read_state(|state| state.minimum_deposit_amount());
    if deposit_amount < minimum_deposit_amount {
        return Err(UpdateBalanceError::ValueTooSmall {
            minimum_deposit_amount,
            deposit_amount,
        });
    }
    let amount_to_mint = deposit_amount
        .checked_sub(read_state(|state| state.manual_deposit_fee()))
        .expect("BUG: deposit amount is less than manual deposit fee");

    mutate_state(|state| {
        process_event(
            state,
            EventType::AcceptedManualDeposit {
                deposit_id,
                deposit_amount,
                amount_to_mint,
            },
            runtime,
        )
    });
    Ok(Deposit {
        deposit_amount,
        amount_to_mint,
    })
}
