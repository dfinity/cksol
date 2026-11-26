use crate::{
    deposit::{
        finalize::{FinalizedQueue, credit_finalized_sweep_round},
        spl::failure::failed_sweep_reason,
    },
    runtime::CanisterRuntime,
    state::{SubmittedSplSweep, SweepMismatch, event::EventType},
    storage::record_failed_spl_sweep,
};

#[cfg(test)]
mod tests;

/// Settles finalized SPL sweeps, crediting matching outcomes and quarantining mismatches.
/// Returns whether progress was made with more finalized sweeps left to process.
pub async fn credit_finalized_spl_sweeps<R: CanisterRuntime>(runtime: &R) -> bool {
    credit_finalized_sweep_round(
        runtime,
        FinalizedQueue::Spl,
        |state| state.spl_deposits().finalized().by_signature(),
        |signature, submitted: SubmittedSplSweep, outcome| {
            let settled = submitted
                .sweep
                .settle(&outcome.transaction, outcome.meta.as_ref())?;
            // The status reported a success, so the metadata must not report a failure.
            if let Some(error) = outcome.meta.as_ref().and_then(|meta| meta.err.as_ref()) {
                return Err(SweepMismatch::TransactionFailed {
                    error: error.to_string(),
                }
                .into());
            }
            Ok(EventType::CreditedSplSweep {
                signature,
                lamports_spent: settled.lamports_spent,
                deposit_id: submitted.sweep.deposit_id(),
                amount_to_mint: submitted.sweep.deposit().amount_to_mint(),
            })
        },
        |signature| EventType::QuarantinedSplSweep { signature },
        "SPL sweep",
    )
    .await
}

/// Settles SPL sweeps recorded as failed at their validated actual cost.
/// A failure releases its reservation only here, never as a zero-cost expiry.
/// Returns whether progress was made with more failed sweeps left to process.
pub async fn settle_failed_spl_sweeps<R: CanisterRuntime>(runtime: &R) -> bool {
    credit_finalized_sweep_round(
        runtime,
        FinalizedQueue::FailedSpl,
        |state| state.spl_deposits().failed().by_signature(),
        |signature, submitted: SubmittedSplSweep, outcome| {
            let settled = submitted
                .sweep
                .settle(&outcome.transaction, outcome.meta.as_ref())?;
            // The status reported a failure, so the metadata must not report a success.
            let Some(error) = outcome.meta.as_ref().and_then(|meta| meta.err.clone()) else {
                return Err(SweepMismatch::TransactionSucceeded.into());
            };
            // The failure is validated and recorded once, so it is counted once.
            record_failed_spl_sweep(
                submitted.sweep.deposit().mint,
                failed_sweep_reason(&error.into()),
            );
            Ok(EventType::FailedSplSweep {
                signature,
                lamports_spent: settled.lamports_spent,
            })
        },
        |signature| EventType::QuarantinedSplSweep { signature },
        "failed SPL sweep",
    )
    .await
}
