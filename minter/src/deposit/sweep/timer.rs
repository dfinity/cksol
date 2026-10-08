use crate::{
    address::{minter_address, minter_public_key},
    constants::MAX_CONCURRENT_RPC_CALLS,
    guard::TimerGuard,
    rpc::{Block, SubmitTransactionError, get_recent_block, submit_transaction},
    runtime::CanisterRuntime,
    sol_transfer::{CreateTransferError, MAX_SIGNATURES, sign_sweep_transaction},
    state::{
        QueuedDeposit, State, Sweep, TaskType,
        audit::process_event,
        event::{EventType, TransactionPurpose},
        mutate_state, read_state,
    },
};
use canlog::log;
use cksol_types::DepositSolId;
use cksol_types_internal::log::Priority;
use itertools::Itertools;
use solana_address::Address;
use solana_signature::Signature;
use std::time::Duration;
use thiserror::Error;

#[cfg(test)]
mod tests;

pub(crate) const MAX_DEPOSITS_PER_SWEEP: usize = MAX_SIGNATURES as usize;

pub async fn sweep_queued_deposits<R: CanisterRuntime>(runtime: R) {
    let _guard = match TimerGuard::new(TaskType::SweepDeposits) {
        Ok(guard) => guard,
        Err(_) => return,
    };

    let sweep = read_state(SweepRound::take_from_queue);
    if sweep.batches.is_empty() {
        return;
    }

    let master_key = match minter_public_key() {
        Ok(key) => key,
        Err(e) => {
            log!(Priority::Info, "Skipping sweep of queued deposits: {e}");
            return;
        }
    };
    let sweep_destination = minter_address(&master_key);

    let block = match get_recent_block(&runtime).await {
        Ok(block) => block,
        Err(e) => {
            log!(
                Priority::Info,
                "Failed to fetch recent blockhash for sweep: {e}"
            );
            return;
        }
    };
    let reschedule = scopeguard::guard(runtime.clone(), |runtime| {
        runtime.set_timer(Duration::ZERO, sweep_queued_deposits);
    });

    futures::future::join_all(sweep.batches.into_iter().map(async |batch| {
        match submit_sweep_transaction(&runtime, batch, block, sweep_destination).await {
            Ok(signature) => log!(Priority::Info, "Submitted sweep transaction {signature}"),
            Err(SweepError::CreateTransactionFailed(e)) => {
                log!(Priority::Error, "Failed to create sweep transaction: {e}")
            }
            Err(SweepError::SubmitTransactionFailed(e)) => log!(
                Priority::Info,
                "Failed to submit sweep transaction (awaiting finalization): {e}"
            ),
        }
    }))
    .await;

    if !sweep.leaves_deposits_queued {
        scopeguard::ScopeGuard::into_inner(reschedule);
    }
}

struct SweepRound {
    batches: Vec<Vec<(DepositSolId, QueuedDeposit)>>,
    leaves_deposits_queued: bool,
}

impl SweepRound {
    const MAX_DEPOSITS_PER_ROUND: usize = MAX_DEPOSITS_PER_SWEEP * MAX_CONCURRENT_RPC_CALLS;

    fn take_from_queue(state: &State) -> Self {
        let mut deposits: Vec<(DepositSolId, QueuedDeposit)> = state
            .deposits()
            .queued()
            .iter()
            .map(|(deposit_id, deposit)| (*deposit_id, *deposit))
            .take(Self::MAX_DEPOSITS_PER_ROUND + 1)
            .collect();
        let leaves_deposits_queued = deposits.len() > Self::MAX_DEPOSITS_PER_ROUND;
        deposits.truncate(Self::MAX_DEPOSITS_PER_ROUND);
        Self {
            batches: deposits
                .into_iter()
                .chunks(MAX_DEPOSITS_PER_SWEEP)
                .into_iter()
                .map(Iterator::collect)
                .collect(),
            leaves_deposits_queued,
        }
    }
}

#[derive(Debug, Error)]
enum SweepError {
    #[error("failed to create transaction: {0}")]
    CreateTransactionFailed(#[from] CreateTransferError),
    #[error("failed to submit transaction: {0}")]
    SubmitTransactionFailed(#[from] SubmitTransactionError),
}

async fn submit_sweep_transaction<R: CanisterRuntime>(
    runtime: &R,
    deposits: Vec<(DepositSolId, QueuedDeposit)>,
    block: Block,
    sweep_destination: Address,
) -> Result<Signature, SweepError> {
    let sweep = Sweep::plan(deposits, sweep_destination);
    let (transaction, signers) = sign_sweep_transaction(runtime, &sweep, block.blockhash).await?;
    let signature = transaction.signatures[0];

    mutate_state(|state| {
        process_event(
            state,
            EventType::SubmittedTransaction {
                signature,
                message: transaction.message.clone().into(),
                signers,
                purpose: TransactionPurpose::SweepDeposit {
                    deposit_ids: sweep.deposits().keys().copied().collect(),
                    block_height: block.block_height,
                },
            },
            runtime,
        )
    });

    submit_transaction(runtime, transaction).await?;

    Ok(signature)
}
