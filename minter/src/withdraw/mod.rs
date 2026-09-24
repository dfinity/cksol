use std::str::FromStr;
use std::time::Duration;

use cksol_types::{WithdrawalError, WithdrawalOk, WithdrawalStatus};
use icrc_ledger_types::icrc1::account::Account;
use solana_address::Address;

use canlog::log;
use cksol_types_internal::log::Priority;

use crate::{
    consolidate::consolidate_deposits,
    constants::MAX_CONCURRENT_RPC_CALLS,
    guard::{TimerGuard, withdrawal_guard},
    ledger::{BurnError, burn},
    rpc::{Block, get_recent_block, submit_transaction},
    runtime::CanisterRuntime,
    sol_transfer::create_signed_batch_withdrawal_transaction,
    state::{
        TaskType,
        audit::process_event,
        event::{EventType, TransactionPurpose, VersionedMessage, WithdrawalRequest},
        mutate_state, read_state,
    },
};

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

    let (batches, more_to_process, num_pending_withdrawals) = read_state(|state| {
        let mut affordable_batches = state.withdrawal_batches().peekable();
        let batches: Vec<Vec<_>> = affordable_batches
            .by_ref()
            .take(MAX_CONCURRENT_RPC_CALLS)
            .collect();
        (
            batches,
            affordable_batches.peek().is_some(),
            state.pending_withdrawal_requests().len(),
        )
    });

    let num_affordable_withdrawals: usize = batches.iter().map(Vec::len).sum();
    if !more_to_process && num_affordable_withdrawals < num_pending_withdrawals {
        log!(
            Priority::Info,
            "Insufficient minter balance for some withdrawal requests, scheduling consolidation"
        );
        runtime.set_timer(Duration::ZERO, consolidate_deposits);
    }

    let reschedule = scopeguard::guard(runtime.clone(), |runtime| {
        runtime.set_timer(Duration::ZERO, process_pending_withdrawals);
    });

    if batches.is_empty() {
        // Nothing to process
        scopeguard::ScopeGuard::into_inner(reschedule);
        return;
    }

    let block = match get_recent_block(&runtime).await {
        Ok(block) => block,
        Err(e) => {
            log!(Priority::Info, "Failed to fetch recent blockhash: {e}");
            return;
        }
    };

    futures::future::join_all(
        batches
            .into_iter()
            .map(async |batch| submit_withdrawal_transaction(&runtime, batch, block).await),
    )
    .await;

    if !more_to_process {
        // All work fits in this round
        scopeguard::ScopeGuard::into_inner(reschedule);
    }
}

async fn submit_withdrawal_transaction<R: CanisterRuntime>(
    runtime: &R,
    requests: Vec<WithdrawalRequest>,
    block: Block,
) {
    let targets: Vec<_> = requests
        .iter()
        .map(|request| {
            let destination = Address::from(request.solana_address);
            (destination, request.amount_to_transfer)
        })
        .collect();

    let (signed_tx, signers) = match create_signed_batch_withdrawal_transaction(
        runtime,
        &targets,
        block.blockhash,
    )
    .await
    {
        Ok(tx) => tx,
        Err(e) => {
            let burn_indices: Vec<_> = requests.iter().map(|r| r.burn_block_index).collect();
            log!(
                Priority::Error,
                "Failed to create batch withdrawal transaction for burn indices {burn_indices:?}: {e}"
            );
            return;
        }
    };

    let signature = signed_tx.signatures[0];
    let message = VersionedMessage::Legacy(signed_tx.message.clone());
    let burn_indices: Vec<_> = requests.iter().map(|r| r.burn_block_index).collect();

    mutate_state(|state| {
        process_event(
            state,
            EventType::SubmittedTransaction {
                signature,
                message,
                signers,
                purpose: TransactionPurpose::WithdrawSol {
                    burn_indices: burn_indices.clone(),
                },
                block_height: block.block_height,
            },
            runtime,
        )
    });

    match submit_transaction(runtime, signed_tx).await {
        Ok(_) => {
            log!(
                Priority::Info,
                "Submitted withdrawal transaction {signature} for burn indices {burn_indices:?}"
            );
        }
        Err(e) => {
            log!(
                Priority::Info,
                "Failed to send withdrawal transaction {signature} (will be resubmitted): {e}"
            );
        }
    }
}

pub fn withdrawal_status(block_index: u64) -> WithdrawalStatus {
    read_state(|s| s.withdrawal_status(block_index))
}
