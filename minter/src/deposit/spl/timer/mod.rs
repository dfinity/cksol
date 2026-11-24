use crate::{
    address::minter_public_key,
    constants::MAX_CONCURRENT_SIGNATURES,
    guard::TimerGuard,
    queue_cursor::QueueCursor,
    rpc::{Block, SubmitTransactionError, get_recent_block, submit_transaction},
    runtime::CanisterRuntime,
    sol_transfer::CreateTransferError,
    spl_transfer::sign_spl_sweep_transaction,
    state::{
        SPL_SWEEP_TRANSACTION_FEE, SplSweep, TaskType,
        audit::process_event,
        event::{EventType, TransactionPurpose},
        mutate_state, read_state,
    },
};
use canlog::log;
use cksol_types::DepositSplId;
use cksol_types_internal::log::Priority;
use sol_rpc_types::CommitmentLevel;
use solana_signature::Signature;
use std::{cell::RefCell, time::Duration};
use thiserror::Error;

pub const SWEEP_SPL_DEPOSITS_DELAY: Duration = Duration::from_secs(60);
const SWEEP_BLOCKHASH_COMMITMENT: CommitmentLevel = CommitmentLevel::Confirmed;
// Each sweep requests two signatures concurrently: the minter and the deposit owner.
const MAX_CONCURRENT_SPL_SWEEPS: usize = MAX_CONCURRENT_SIGNATURES / 2;

thread_local! {
    static CURSOR: RefCell<QueueCursor<DepositSplId>> = const { RefCell::new(QueueCursor::new()) };
}

/// Submits bounded rounds of independent transfers. Unresolved sweeps keep their fees reserved.
pub async fn sweep_queued_spl_deposits<R: CanisterRuntime>(runtime: R) {
    let _guard = match TimerGuard::new(TaskType::SweepSplDeposits) {
        Ok(guard) => guard,
        Err(_) => return,
    };
    if read_state(|state| {
        state.spl_deposits().queued().is_empty()
            || state.spl_sweep_allowance() < SPL_SWEEP_TRANSACTION_FEE
    }) {
        return;
    }
    let master_key = match minter_public_key() {
        Ok(key) => key,
        Err(error) => {
            log!(Priority::Info, "Skipping SPL sweeps: {error}");
            return;
        }
    };
    let sweeps = read_state(|state| {
        let limit = (state.spl_sweep_allowance() / SPL_SWEEP_TRANSACTION_FEE)
            .min(MAX_CONCURRENT_SPL_SWEEPS as u64) as usize;
        CURSOR.with(|cursor| {
            cursor
                .borrow()
                .round(state.spl_deposits().queued(), limit)
                .into_iter()
                .map(|(id, deposit)| {
                    let token = state
                        .supported_spl_token(&deposit.mint)
                        .expect("BUG: queued SPL mint is unregistered");
                    SplSweep::plan(*id, deposit.clone(), token, &master_key)
                        .expect("BUG: queued SPL deposit has an invalid source")
                })
                .collect::<Vec<_>>()
        })
    });
    let block = match get_recent_block(&runtime, SWEEP_BLOCKHASH_COMMITMENT).await {
        Ok(block) => block,
        Err(error) => {
            log!(
                Priority::Info,
                "Failed to fetch recent blockhash for SPL sweeps: {error}"
            );
            return;
        }
    };
    let submissions = sweeps.iter().map(async |sweep| {
        match submit_spl_sweep_transaction(&runtime, sweep, block).await {
            Ok(signature) => log!(
                Priority::Info,
                "Submitted SPL sweep transaction {signature}"
            ),
            Err(SweepError::CreateTransactionFailed(error)) => {
                log!(
                    Priority::Error,
                    "Failed to create SPL sweep transaction: {error}"
                )
            }
            Err(SweepError::SubmitTransactionFailed(error)) => log!(
                Priority::Info,
                "Failed to submit SPL sweep transaction (awaiting finalization): {error}"
            ),
            Err(SweepError::BalanceNoLongerCovers) => log!(
                Priority::Info,
                "The balance no longer covers an SPL sweep, leaving its deposit queued"
            ),
        }
    });
    futures::future::join_all(submissions).await;
}

#[derive(Debug, Error)]
enum SweepError {
    #[error("failed to create transaction: {0}")]
    CreateTransactionFailed(#[from] CreateTransferError),
    #[error("failed to submit transaction: {0}")]
    SubmitTransactionFailed(#[from] SubmitTransactionError),
    #[error("the balance no longer covers the sweep")]
    BalanceNoLongerCovers,
}

async fn submit_spl_sweep_transaction<R: CanisterRuntime>(
    runtime: &R,
    sweep: &SplSweep,
    block: Block,
) -> Result<Signature, SweepError> {
    // The funds may have changed while the recent block was read.
    if !read_state(|state| state.covers_spl_sweep(sweep)) {
        return Err(SweepError::BalanceNoLongerCovers);
    }
    // Advance before the signing call, so a failing deposit does not block the next round.
    CURSOR.with(|cursor| cursor.borrow_mut().advance(sweep.deposit_id()));
    let (transaction, signers) =
        sign_spl_sweep_transaction(runtime, sweep, block.blockhash).await?;
    // A withdrawal may have taken part of the balance during the signing.
    if !read_state(|state| state.covers_spl_sweep(sweep)) {
        return Err(SweepError::BalanceNoLongerCovers);
    }
    let signature = transaction.signatures[0];
    mutate_state(|state| {
        process_event(
            state,
            EventType::SubmittedTransaction {
                signature,
                message: transaction.message.clone().into(),
                signers,
                purpose: TransactionPurpose::SweepSplDeposit {
                    deposit_id: sweep.deposit_id(),
                    block_height: block.block_height,
                },
            },
            runtime,
        )
    });
    submit_transaction(runtime, transaction, SWEEP_BLOCKHASH_COMMITMENT).await?;
    Ok(signature)
}
