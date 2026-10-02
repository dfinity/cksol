use crate::{
    constants::MAX_CONCURRENT_RPC_CALLS,
    rpc::get_transaction,
    runtime::CanisterRuntime,
    state::{
        Sweep, SweepSettlementError, audit::process_event, event::EventType, mutate_state,
        read_state,
    },
    storage::{FailedCreditReason, record_failed_credit_attempt},
};
use canlog::log;
use cksol_types_internal::log::Priority;
use solana_signature::Signature;

#[cfg(test)]
mod tests;

/// Credits the deposits of the sweep transactions that have been finalized successfully.
///
/// Each sweep is settled against the plan it was submitted with, so that only an outcome
/// matching exactly what the minter built is credited. A sweep whose outcome cannot be read
/// is left finalized and retried on the next run, and so is a sweep whose outcome does not
/// match its plan.
///
/// Returns whether the finalization timer must run again immediately, which is the case
/// only when this round credited a sweep and finalized sweeps are left. A round whose
/// fetches all failed is retried at the timer interval instead of in a hot loop.
pub async fn credit_finalized_sweeps<R: CanisterRuntime>(runtime: &R) -> bool {
    let (finalized_count, round): (usize, Vec<(Signature, Sweep)>) = read_state(|state| {
        let finalized = state.deposits().finalized();
        (
            finalized.len(),
            finalized
                .iter()
                .take(MAX_CONCURRENT_RPC_CALLS)
                .map(|(signature, sweep)| (*signature, sweep.clone()))
                .collect(),
        )
    });
    if round.is_empty() {
        return false;
    }

    let outcomes = futures::future::join_all(
        round
            .iter()
            .map(|(signature, _)| get_transaction(runtime, *signature)),
    )
    .await;

    let mut credited_count = 0;
    for ((signature, sweep), outcome) in round.into_iter().zip(outcomes) {
        let outcome = match outcome {
            Ok(Some(outcome)) => outcome,
            Ok(None) => {
                log!(
                    Priority::Info,
                    "Finalized sweep {signature} was not returned by getTransaction, retrying later"
                );
                record_failed_credit_attempt(FailedCreditReason::NotFound);
                continue;
            }
            Err(e) => {
                log!(
                    Priority::Info,
                    "Failed to fetch finalized sweep {signature}: {e}, retrying later"
                );
                record_failed_credit_attempt(FailedCreditReason::RpcError);
                continue;
            }
        };
        let settled = match sweep.settle(&outcome) {
            Ok(settled) => settled,
            Err(SweepSettlementError::Unreadable(e)) => {
                log!(
                    Priority::Info,
                    "Could not read the outcome of sweep {signature}: {e}, retrying later"
                );
                record_failed_credit_attempt(FailedCreditReason::Unreadable);
                continue;
            }
            Err(SweepSettlementError::Mismatch(e)) => {
                log!(
                    Priority::Error,
                    "The outcome of sweep {signature} does not match its plan: {e}, retrying later"
                );
                record_failed_credit_attempt(FailedCreditReason::Mismatch);
                continue;
            }
        };
        let event = EventType::CreditedSweep {
            signature,
            amount_received: settled.amount_received(),
            mints: settled.into_mints(),
        };
        mutate_state(|state| process_event(state, event, runtime));
        credited_count += 1;
    }

    credited_count > 0 && credited_count < finalized_count
}
