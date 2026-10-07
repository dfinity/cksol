use crate::{
    address::minter_address,
    constants::MAX_CONCURRENT_RPC_CALLS,
    monitor::MIN_REBROADCAST_AGE,
    rpc::{
        TransactionOutcome, get_submitted_transaction_outcome,
        submit_transaction_skipping_preflight,
    },
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
use solana_address::Address;
use solana_hash::Hash;
use solana_message::Message;
use solana_signature::Signature;
use solana_transaction::Transaction;
use std::time::Duration;

/// Decides every in-flight withdrawal transaction from a finalized read of its nonce
/// account: a landed transaction is finalized with the outcome `getTransaction` reports,
/// and a transaction that has not landed is re-broadcast once old enough.
pub(super) async fn check_withdrawal_transactions<R: CanisterRuntime>(runtime: &R) {
    let Some(minter_address) = read_state(|state| state.minter_public_key().map(minter_address))
    else {
        return;
    };
    let withdrawals = in_flight_withdrawals();
    let mut num_unresolved_outcomes = 0;
    for batch in withdrawals.chunks(MAX_CONCURRENT_RPC_CALLS) {
        num_unresolved_outcomes += futures::future::join_all(
            batch
                .iter()
                .map(|withdrawal| check_withdrawal(runtime, withdrawal, minter_address)),
        )
        .await
        .into_iter()
        .filter(|check| *check == WithdrawalCheck::UnresolvedOutcome)
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

#[derive(PartialEq)]
enum WithdrawalCheck {
    NoOutcome,
    UnresolvedOutcome,
    Finalized,
}

async fn check_withdrawal<R: CanisterRuntime>(
    runtime: &R,
    withdrawal: &InFlightWithdrawal,
    minter_address: Address,
) -> WithdrawalCheck {
    let InFlightWithdrawal {
        signature,
        nonce_account,
        nonce_value,
        ..
    } = withdrawal;
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
            return WithdrawalCheck::NoOutcome;
        }
    };
    let nonce_read = read_state(|state| {
        state
            .nonce_pool()
            .classify_read(nonce_account, nonce_value, &read_nonce_value)
    });
    match nonce_read {
        NonceRead::Unchanged => {
            if withdrawal.is_old_enough_to_rebroadcast(runtime.time()) {
                rebroadcast(runtime, withdrawal).await;
            }
            WithdrawalCheck::NoOutcome
        }
        NonceRead::Stale => {
            log!(
                Priority::Info,
                "Stale read of nonce account {nonce_account} of withdrawal transaction {signature}, retrying next round"
            );
            WithdrawalCheck::NoOutcome
        }
        NonceRead::Advanced => finalize_landed_withdrawal(runtime, withdrawal).await,
    }
}

async fn finalize_landed_withdrawal<R: CanisterRuntime>(
    runtime: &R,
    withdrawal: &InFlightWithdrawal,
) -> WithdrawalCheck {
    let signature = withdrawal.signature;
    let event = match get_submitted_transaction_outcome(runtime, signature, &withdrawal.message)
        .await
    {
        Ok(Some(TransactionOutcome::Succeeded)) => {
            log!(Priority::Info, "Transaction {signature} finalized");
            EventType::SucceededTransaction { signature }
        }
        Ok(Some(TransactionOutcome::Failed(error))) => {
            log!(
                Priority::Error,
                "Transaction {signature} finalized with on-chain error: {error:?}"
            );
            EventType::FailedTransaction { signature }
        }
        Ok(None) => {
            log!(
                Priority::Info,
                "Withdrawal transaction {signature} landed but was not found, retrying next round"
            );
            return WithdrawalCheck::UnresolvedOutcome;
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
            return WithdrawalCheck::UnresolvedOutcome;
        }
    };
    mutate_state(|state| process_event(state, event, runtime));
    WithdrawalCheck::Finalized
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
