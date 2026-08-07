use crate::{
    consolidate::consolidate_deposits,
    guard::{TimerGuard, withdrawal_guard},
    ledger::{BurnError, burn},
    rpc_executor::{WorkItem, enqueue, execute_rpc_queue},
    runtime::CanisterRuntime,
    sol_transfer::MAX_WITHDRAWALS_PER_TX,
    state::{
        TaskType,
        audit::process_event,
        event::{EventType, WithdrawalRequest},
        mutate_state, read_state,
    },
};
use canlog::log;
use cksol_types::{WithdrawalError, WithdrawalOk, WithdrawalStatus};
use cksol_types_internal::log::Priority;
use icrc_ledger_types::icrc1::account::Account;
use itertools::Itertools;
use solana_address::Address;
use std::{str::FromStr, time::Duration};

pub const WITHDRAWAL_PROCESSING_DELAY: Duration = Duration::from_mins(1);

#[cfg(test)]
mod tests;

pub async fn withdraw<R: CanisterRuntime>(
    runtime: &R,
    from: Account,
    amount_to_burn: u64,
    address: String,
) -> Result<WithdrawalOk, WithdrawalError> {
    let minimum_withdrawal_amount = read_state(|s| s.minimum_withdrawal_amount());
    if amount_to_burn < minimum_withdrawal_amount {
        return Err(WithdrawalError::ValueTooSmall {
            minimum_withdrawal_amount,
            withdrawal_amount: amount_to_burn,
        });
    }

    let _guard = withdrawal_guard(from)?;

    let solana_address = Address::from_str(&address)
        .map_err(|e| WithdrawalError::MalformedAddress(e.to_string()))?;

    let minter_account: Account = runtime.canister_self().into();
    let block_index = burn(
        runtime,
        minter_account,
        from,
        amount_to_burn,
        solana_address,
    )
    .await
    .map_err(|e| match e {
        BurnError::TemporarilyUnavailable(msg) => WithdrawalError::TemporarilyUnavailable(msg),
        BurnError::InsufficientFunds { balance } => WithdrawalError::InsufficientFunds { balance },
        BurnError::InsufficientAllowance { allowance } => {
            WithdrawalError::InsufficientAllowance { allowance }
        }
    })?;

    let withdrawal_fee = read_state(|s| s.withdrawal_fee());
    let amount_to_transfer = amount_to_burn
        .checked_sub(withdrawal_fee)
        .expect("BUG: burned amount must be >= withdrawal fee");
    mutate_state(|s| {
        process_event(
            s,
            EventType::AcceptedWithdrawalRequest(WithdrawalRequest {
                account: from,
                solana_address: solana_address.to_bytes(),
                burn_block_index: block_index.into(),
                amount_to_transfer,
                burned_amount: amount_to_burn,
            }),
            runtime,
        )
    });
    log!(
        Priority::Info,
        "Accepted withdrawal request from {from:?}: burned {amount_to_burn} lamports, queued withdrawal of {amount_to_transfer} lamports to {solana_address} (burn block index {block_index})"
    );

    Ok(WithdrawalOk { block_index })
}

pub async fn process_pending_withdrawals<R: CanisterRuntime>(runtime: R) {
    let _guard = match TimerGuard::new(TaskType::WithdrawalProcessing) {
        Ok(guard) => guard,
        Err(_) => {
            log!(
                Priority::Info,
                "failed to obtain WithdrawalProcessing guard, exiting"
            );
            return;
        }
    };

    let (affordable_requests, num_pending_withdrawals) = read_state(|state| {
        let mut available_balance = state.balance();
        let pending = state.pending_withdrawal_requests();

        let affordable: Vec<_> = pending
            .values()
            .take_while(|r| {
                if available_balance >= r.request.amount_to_transfer {
                    available_balance -= r.request.amount_to_transfer;
                    true
                } else {
                    false
                }
            })
            .map(|t| t.request.clone())
            .collect();

        (affordable, pending.len())
    });

    if affordable_requests.len() < num_pending_withdrawals {
        log!(
            Priority::Info,
            "Insufficient minter balance for some withdrawal requests, scheduling consolidation"
        );
        runtime.set_timer(Duration::ZERO, consolidate_deposits);
    }

    if affordable_requests.is_empty() {
        return;
    }

    for batch in affordable_requests
        .into_iter()
        .chunks(MAX_WITHDRAWALS_PER_TX)
        .into_iter()
    {
        enqueue(WorkItem::SubmitWithdrawalBatch(batch.collect()));
    }

    runtime.set_timer(Duration::ZERO, execute_rpc_queue);
}

pub fn withdrawal_status(block_index: u64) -> WithdrawalStatus {
    read_state(|s| s.withdrawal_status(block_index))
}
