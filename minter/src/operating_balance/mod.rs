use crate::{
    constants::LEDGER_DEDUPLICATION_WINDOW,
    guard::TimerGuard,
    runtime::CanisterRuntime,
    state::{
        TaskType, audit::process_event, event::EventType, mutate_state,
        operating_balance::PendingOperatingRefill, read_state,
    },
    storage::{
        OperatingRefillErrorReason, TopUpBalanceRead, record_operating_refill_error,
        with_unstable_metrics_mut,
    },
};
use candid::Principal;
use canlog::log;
use cksol_types::{BurnMemo, Memo};
use cksol_types_internal::log::Priority;
use icrc_ledger_types::icrc1::{
    account::Account,
    transfer::{TransferArg, TransferError},
};
use num_traits::ToPrimitive;
use std::time::Duration;

#[cfg(test)]
mod tests;

pub const OPERATING_REFILL_DELAY: Duration = Duration::from_mins(1);

// Part of the durable meaning of RequestedOperatingRefill; do not change on upgrade.
const OPERATING_TOP_UP_SUBACCOUNT: [u8; 32] = {
    let mut bytes = [0; 32];
    bytes[30] = 0xbe;
    bytes[31] = 0xef;
    bytes
};

pub fn operating_top_up_account(minter: Principal) -> Account {
    Account {
        owner: minter,
        subaccount: Some(OPERATING_TOP_UP_SUBACCOUNT),
    }
}

/// Starts at most one refill and retries an existing intent even when refills are disabled.
/// An unresolved or quarantined burn never credits operating funds or starts another burn.
pub async fn refill_operating_balance<R: CanisterRuntime>(runtime: R) {
    let _guard = match TimerGuard::new(TaskType::OperatingRefill) {
        Ok(guard) => guard,
        Err(_) => return,
    };
    if let Some(pending) = read_state(|s| s.operating_balance().pending()) {
        if !pending.quarantined {
            burn_refill(&runtime, pending, false).await;
        }
        return;
    }
    if !read_state(|s| s.operating_balance().config().enabled) {
        return;
    }

    let client = read_state(|s| s.ledger_client(runtime.inter_canister_call_runtime()));
    let balance = match client
        .balance_of(operating_top_up_account(runtime.canister_self()))
        .await
    {
        Ok(balance) => match balance.0.to_u64() {
            Some(balance) => balance,
            None => {
                record_operating_refill_error(OperatingRefillErrorReason::BalanceOverflow);
                log!(
                    Priority::Error,
                    "Operating top-up balance exceeds u64; no refill started"
                );
                return;
            }
        },
        Err(error) => {
            record_operating_refill_error(OperatingRefillErrorReason::BalanceRead);
            log!(
                Priority::Info,
                "Failed to read operating top-up balance: {error}"
            );
            return;
        }
    };
    with_unstable_metrics_mut(|m| {
        m.operating_top_up_balance = Some(TopUpBalanceRead {
            balance,
            read_at: runtime.time(),
        });
    });
    // Re-evaluate after the await; upgrade settings and spendable funds may have changed.
    let amount = read_state(|s| s.operating_balance().refill_amount()).min(balance);
    if amount == 0 {
        return;
    }
    let refill_id = read_state(|s| s.operating_balance().next_refill_id());
    let created_at_time = runtime.time();
    record_event(
        &runtime,
        EventType::RequestedOperatingRefill {
            refill_id,
            amount,
            created_at_time,
        },
    );
    let pending = read_state(|s| s.operating_balance().pending().unwrap());
    burn_refill(&runtime, pending, true).await;
}

async fn burn_refill<R: CanisterRuntime>(
    runtime: &R,
    pending: PendingOperatingRefill,
    first_attempt: bool,
) {
    let refill_id = pending.refill_id;
    let age = Duration::from_nanos(runtime.time().saturating_sub(pending.created_at_time));
    if age > LEDGER_DEDUPLICATION_WINDOW {
        quarantine(
            runtime,
            refill_id,
            OperatingRefillErrorReason::DeduplicationExpired,
        );
        return;
    }
    let client = read_state(|s| s.ledger_client(runtime.inter_canister_call_runtime()));
    let result = client
        .transfer(burn_arguments(runtime.canister_self(), pending))
        .await;
    let burn_block_index = match result {
        Ok(Ok(index)) => *index.get(),
        Ok(Err(TransferError::Duplicate { duplicate_of })) => duplicate_of
            .0
            .to_u64()
            .expect("BUG: ledger block index exceeds u64"),
        Ok(Err(TransferError::TooOld)) => {
            quarantine(runtime, refill_id, OperatingRefillErrorReason::TooOld);
            return;
        }
        Ok(Err(
            error @ (TransferError::BadFee { .. }
            | TransferError::BadBurn { .. }
            | TransferError::InsufficientFunds { .. }),
        )) => {
            // On a retry, rejection of this call alone does not prove the earlier burn failed.
            if first_attempt {
                record_operating_refill_error(OperatingRefillErrorReason::Rejected);
                record_event(
                    runtime,
                    EventType::FailedOperatingRefill {
                        refill_id,
                        reason: format!("{error:?}"),
                    },
                );
            } else {
                quarantine(
                    runtime,
                    refill_id,
                    OperatingRefillErrorReason::RejectedAfterUnknownOutcome,
                );
            }
            return;
        }
        Ok(Err(error)) => {
            record_operating_refill_error(match error {
                TransferError::CreatedInFuture { .. } => {
                    OperatingRefillErrorReason::CreatedInFuture
                }
                _ => OperatingRefillErrorReason::LedgerError,
            });
            log!(
                Priority::Info,
                "Operating refill {refill_id} remains pending: {error:?}"
            );
            return;
        }
        Err(error) => {
            record_operating_refill_error(OperatingRefillErrorReason::CallError);
            log!(
                Priority::Info,
                "Operating refill {refill_id} remains pending after call failure: {error}"
            );
            return;
        }
    };
    record_event(
        runtime,
        EventType::CompletedOperatingRefill {
            refill_id,
            burn_block_index,
        },
    );
}

fn burn_arguments(minter: Principal, pending: PendingOperatingRefill) -> TransferArg {
    TransferArg {
        from_subaccount: Some(OPERATING_TOP_UP_SUBACCOUNT),
        to: minter.into(),
        fee: None,
        created_at_time: Some(pending.created_at_time),
        memo: Some(
            Memo::from(BurnMemo::OperatingRefill {
                refill_id: pending.refill_id,
            })
            .into(),
        ),
        amount: pending.amount.into(),
    }
}

fn record_event<R: CanisterRuntime>(runtime: &R, event: EventType) {
    mutate_state(|s| process_event(s, event, runtime));
}

fn quarantine<R: CanisterRuntime>(runtime: &R, refill_id: u64, reason: OperatingRefillErrorReason) {
    record_operating_refill_error(reason);
    log!(
        Priority::Error,
        "Quarantining operating refill {refill_id}: {}; inspect the ledger before manual resolution",
        reason.label()
    );
    record_event(runtime, EventType::QuarantinedOperatingRefill { refill_id });
}
