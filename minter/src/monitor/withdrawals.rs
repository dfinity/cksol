use crate::{
    address::minter_address,
    constants::MAX_CONCURRENT_RPC_CALLS,
    monitor::MIN_REBROADCAST_AGE,
    rpc::{FetchedTransaction, get_transaction, submit_transaction_skipping_preflight},
    runtime::CanisterRuntime,
    state::{
        MinterTransaction, NonceRead,
        audit::process_event,
        event::{EventType, VersionedMessage},
        mutate_state, read_state,
    },
    storage::with_unstable_metrics_mut,
    withdraw::nonce::read_verified_nonce,
};
use canlog::log;
use cksol_types_internal::log::Priority;
use itertools::Itertools;
use solana_address::Address;
use solana_hash::Hash;
use solana_message::Message;
use solana_signature::Signature;
use solana_transaction::Transaction;
use std::time::Duration;

#[derive(Default)]
pub(super) struct InFlightNonces {
    unchanged: Vec<InFlightWithdrawal>,
    advanced: Vec<InFlightWithdrawal>,
}

pub(super) async fn read_in_flight_nonces<R: CanisterRuntime>(runtime: &R) -> InFlightNonces {
    let mut nonces = InFlightNonces::default();
    let Some(minter_address) = read_state(|state| state.minter_public_key().map(minter_address))
    else {
        return nonces;
    };
    let batches: Vec<Vec<_>> = in_flight_withdrawals()
        .into_iter()
        .chunks(MAX_CONCURRENT_RPC_CALLS)
        .into_iter()
        .map(Iterator::collect)
        .collect();
    for batch in batches {
        let reads = futures::future::join_all(
            batch
                .into_iter()
                .map(|withdrawal| read_nonce(runtime, withdrawal, minter_address)),
        )
        .await;
        for (withdrawal, nonce_read) in reads {
            match nonce_read {
                Some(NonceRead::Unchanged) => nonces.unchanged.push(withdrawal),
                Some(NonceRead::Advanced) => nonces.advanced.push(withdrawal),
                Some(NonceRead::Stale) | None => {}
            }
        }
    }
    nonces
}

pub(super) async fn resubmit_transactions_batch<R: CanisterRuntime>(
    runtime: &R,
    nonces: &InFlightNonces,
) {
    let now = runtime.time();
    let to_rebroadcast: Vec<_> = nonces
        .unchanged
        .iter()
        .filter(|withdrawal| withdrawal.is_old_enough_to_rebroadcast(now))
        .collect();
    for batch in to_rebroadcast.chunks(MAX_CONCURRENT_RPC_CALLS) {
        futures::future::join_all(
            batch
                .iter()
                .map(|withdrawal| rebroadcast(runtime, withdrawal)),
        )
        .await;
    }
}

pub(super) async fn finalize_transactions_batch<R: CanisterRuntime>(
    runtime: &R,
    nonces: &InFlightNonces,
) {
    let mut num_unresolved_outcomes = 0;
    for batch in nonces.advanced.chunks(MAX_CONCURRENT_RPC_CALLS) {
        num_unresolved_outcomes += futures::future::join_all(
            batch
                .iter()
                .map(|withdrawal| finalize_landed_withdrawal(runtime, withdrawal)),
        )
        .await
        .into_iter()
        .filter(|finalization| *finalization == Finalization::UnresolvedOutcome)
        .count();
    }
    with_unstable_metrics_mut(|metrics| {
        metrics.withdrawal_transactions_with_unresolved_outcome = num_unresolved_outcomes as u64
    });
}

struct InFlightWithdrawal {
    signature: Signature,
    message: Message,
    nonce_account: Address,
    nonce_value: Hash,
    submitted_at: u64,
}

impl InFlightWithdrawal {
    fn signed_transaction(&self) -> Transaction {
        Transaction {
            signatures: vec![self.signature],
            message: self.message.clone(),
        }
    }

    fn is_old_enough_to_rebroadcast(&self, now: u64) -> bool {
        Duration::from_nanos(now.saturating_sub(self.submitted_at)) >= MIN_REBROADCAST_AGE
    }
}

fn in_flight_withdrawals() -> Vec<InFlightWithdrawal> {
    read_state(|state| {
        state
            .submitted_transactions()
            .iter()
            .filter_map(|(signature, transaction)| match transaction {
                MinterTransaction::Withdrawal {
                    message: VersionedMessage::Legacy(message),
                    nonce_account,
                    nonce_value,
                    submitted_at,
                } => Some(InFlightWithdrawal {
                    signature: *signature,
                    message: message.clone(),
                    nonce_account: *nonce_account,
                    nonce_value: *nonce_value,
                    submitted_at: *submitted_at,
                }),
                MinterTransaction::SweepDeposit { .. } => None,
            })
            .collect()
    })
}

async fn read_nonce<R: CanisterRuntime>(
    runtime: &R,
    withdrawal: InFlightWithdrawal,
    minter_address: Address,
) -> (InFlightWithdrawal, Option<NonceRead>) {
    let InFlightWithdrawal {
        signature,
        nonce_account,
        nonce_value,
        ..
    } = &withdrawal;
    let read_nonce_value = match read_verified_nonce(runtime, *nonce_account, minter_address).await
    {
        Ok(read_nonce_value) => read_nonce_value,
        Err(e) => {
            let priority = if e.is_rpc_failure() {
                Priority::Info
            } else {
                Priority::Error
            };
            log!(
                priority,
                "Failed to read nonce account {nonce_account} of withdrawal transaction {signature}, retrying next round: {e}"
            );
            return (withdrawal, None);
        }
    };
    let nonce_read = read_state(|state| {
        state
            .nonce_pool()
            .classify_read(nonce_account, nonce_value, &read_nonce_value)
    });
    if nonce_read == NonceRead::Stale {
        log!(
            Priority::Info,
            "Stale read of nonce account {nonce_account} of withdrawal transaction {signature}, retrying next round"
        );
    }
    (withdrawal, Some(nonce_read))
}

#[derive(PartialEq)]
enum Finalization {
    Finalized,
    UnresolvedOutcome,
}

async fn finalize_landed_withdrawal<R: CanisterRuntime>(
    runtime: &R,
    withdrawal: &InFlightWithdrawal,
) -> Finalization {
    let signature = withdrawal.signature;
    let event = match get_transaction(runtime, signature).await {
        Ok(Some(FetchedTransaction {
            meta: Some(meta), ..
        })) => match meta.err {
            None => {
                log!(Priority::Info, "Transaction {signature} finalized");
                EventType::SucceededTransaction { signature }
            }
            Some(error) => {
                log!(
                    Priority::Error,
                    "Transaction {signature} finalized with on-chain error: {error:?}"
                );
                EventType::FailedTransaction { signature }
            }
        },
        Ok(Some(FetchedTransaction { meta: None, .. })) => {
            log!(
                Priority::Error,
                "Withdrawal transaction {signature} landed without status metadata, retrying next round"
            );
            return Finalization::UnresolvedOutcome;
        }
        Ok(None) => {
            log!(
                Priority::Info,
                "Withdrawal transaction {signature} landed but was not found, retrying next round"
            );
            return Finalization::UnresolvedOutcome;
        }
        Err(e) => {
            let priority = if e.is_response_untrustworthy() {
                Priority::Error
            } else {
                Priority::Info
            };
            log!(
                priority,
                "Failed to fetch landed withdrawal transaction {signature}, retrying next round: {e}"
            );
            return Finalization::UnresolvedOutcome;
        }
    };
    mutate_state(|state| process_event(state, event, runtime));
    Finalization::Finalized
}

async fn rebroadcast<R: CanisterRuntime>(runtime: &R, withdrawal: &InFlightWithdrawal) {
    let signature = withdrawal.signature;
    with_unstable_metrics_mut(|m| m.withdrawal_transaction_rebroadcasts += 1);
    match submit_transaction_skipping_preflight(runtime, withdrawal.signed_transaction()).await {
        Ok(_) => log!(
            Priority::Info,
            "Re-broadcast withdrawal transaction {signature}"
        ),
        Err(e) => log!(
            Priority::Info,
            "Failed to re-broadcast withdrawal transaction {signature} (will retry next round): {e}"
        ),
    }
}
