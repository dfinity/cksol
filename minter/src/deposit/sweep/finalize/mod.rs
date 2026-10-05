use crate::{
    constants::MAX_CONCURRENT_RPC_CALLS,
    rpc::get_transaction,
    runtime::CanisterRuntime,
    state::{
        Sweep, SweepSettlementError, audit::process_event, event::EventType, mutate_state,
        read_state,
    },
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
/// is left finalized and retried on the next run, while a sweep whose outcome does not match
/// its plan is quarantined instead of credited.
///
/// Returns whether the finalization timer must run again immediately, which is the case
/// only when this round credited or quarantined a sweep and finalized sweeps are left. A
/// round whose fetches all failed is retried at the timer interval instead of in a hot loop.
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

    let mut settled_count = 0;
    for ((signature, sweep), outcome) in round.into_iter().zip(outcomes) {
        let outcome = match outcome {
            Ok(Some(outcome)) => outcome,
            Ok(None) => {
                log!(
                    Priority::Info,
                    "Finalized sweep {signature} was not returned by getTransaction, retrying later"
                );
                continue;
            }
            Err(e) => {
                let priority = if e.is_response_untrustworthy() {
                    Priority::Error
                } else {
                    Priority::Info
                };
                log!(
                    priority,
                    "Failed to fetch finalized sweep {signature}: {e}, retrying later"
                );
                continue;
            }
        };
        let event = match sweep.settle(&outcome) {
            Ok(settled) => EventType::CreditedSweep {
                signature,
                amount_received: settled.amount_received(),
                mints: settled.into_mints(),
            },
            Err(SweepSettlementError::Unreadable(e)) => {
                log!(
                    Priority::Info,
                    "Could not read the outcome of sweep {signature}: {e}, retrying later"
                );
                continue;
            }
            Err(SweepSettlementError::Mismatch(e)) => {
                log!(
                    Priority::Error,
                    "Quarantining the deposits of sweep {signature}: {e}"
                );
                EventType::QuarantinedSweep { signature }
            }
        };
        mutate_state(|state| process_event(state, event, runtime));
        settled_count += 1;
    }

    settled_count > 0 && settled_count < finalized_count
}
