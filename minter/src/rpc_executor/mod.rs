use crate::{
    address::{account_address, derivation_path, lazy_get_schnorr_master_key},
    constants::{MAX_CONCURRENT_RPC_CALLS, MAX_TRANSACTIONS_PER_ACCOUNT},
    guard::TimerGuard,
    numeric::LedgerMintIndex,
    rpc::{
        get_recent_slot_and_blockhash, get_signature_statuses, get_signatures_for_address,
        submit_transaction,
    },
    runtime::CanisterRuntime,
    signer::sign_bytes,
    sol_transfer::{
        MAX_SIGNATURES, create_signed_batch_withdrawal_transaction,
        create_signed_consolidation_transaction,
    },
    state::{
        TaskType,
        audit::process_event,
        event::{EventType, TransactionPurpose, VersionedMessage, WithdrawalRequest},
        mutate_state,
    },
};
use canlog::log;
use cksol_types_internal::log::Priority;
use icrc_ledger_types::icrc1::account::Account;
use sol_rpc_types::{CommitmentLevel, GetSignaturesForAddressParams, Lamport, Slot};
use solana_address::Address;
use solana_hash::Hash;
use solana_signature::Signature;
use solana_transaction::Transaction;
use solana_transaction_status_client_types::TransactionConfirmationStatus;
use std::{
    cell::RefCell,
    collections::{BTreeMap, VecDeque},
    time::Duration,
};

#[cfg(test)]
mod tests;

/// Maximum number of signatures in a single `getSignatureStatuses` RPC call.
/// See <https://solana.com/docs/rpc/http/getsignaturestatuses>
pub const MAX_SIGNATURES_PER_STATUS_CHECK: usize = 256;

/// Maximum number of transfers included in a single consolidation transaction.
pub const MAX_TRANSFERS_PER_CONSOLIDATION: usize = MAX_SIGNATURES as usize;

/// How many slots a blockhash remains valid after it was included in a transaction.
/// A transaction whose blockhash is more than this many slots old is considered expired.
pub const MAX_BLOCKHASH_AGE: Slot = 150;

thread_local! {
    static WORK_QUEUE: RefCell<VecDeque<WorkItem>> = RefCell::default();

    /// Pending deposit signatures discovered by the automated polling loop,
    /// keyed by the account they belong to.
    static PENDING_SIGNATURES: RefCell<BTreeMap<Account, VecDeque<Signature>>> =
        RefCell::default();
}

/// A unit of work to be executed by [`execute_rpc_queue`].
#[derive(Clone, Debug)]
pub enum WorkItem {
    /// Check the on-chain status of a batch of submitted transactions.
    CheckSignatureStatuses {
        /// Signatures to check.
        signatures: Vec<Signature>,
        /// Slot at which each transaction was submitted, used to detect expiry.
        submitted_slots: BTreeMap<Signature, Slot>,
    },
    /// Poll a monitored address for new deposit transaction signatures.
    PollMonitoredAddress(Account),
    /// Submit a consolidation transaction for a batch of minted deposits.
    SubmitConsolidationBatch(Vec<(Account, (Lamport, Vec<LedgerMintIndex>))>),
    /// Submit a batch withdrawal transaction.
    SubmitWithdrawalBatch(Vec<WithdrawalRequest>),
    /// Re-sign and resubmit an expired transaction with a fresh blockhash.
    ResubmitTransaction {
        old_signature: Signature,
        message: VersionedMessage,
        signers: Vec<Account>,
    },
}

impl WorkItem {
    /// Returns `true` for items that require a current slot and recent blockhash
    /// from the executor before they can be executed.
    fn needs_slot_and_blockhash(&self) -> bool {
        !matches!(self, WorkItem::PollMonitoredAddress(_))
    }
}

/// Push a work item onto the back of the executor queue.
pub fn enqueue(item: WorkItem) {
    WORK_QUEUE.with(|q| q.borrow_mut().push_back(item));
}

/// Drain up to `max` items from the front of the executor queue.
fn dequeue_batch(max: usize) -> Vec<WorkItem> {
    WORK_QUEUE.with(|q| {
        let mut q = q.borrow_mut();
        let n = max.min(q.len());
        q.drain(..n).collect()
    })
}

/// Returns `true` if the executor queue is empty.
pub fn queue_is_empty() -> bool {
    WORK_QUEUE.with(|q| q.borrow().is_empty())
}

/// Consume up to [`MAX_CONCURRENT_RPC_CALLS`] items from the work queue and
/// execute them concurrently.
///
/// If any queued item requires a recent slot or blockhash, one
/// `getLatestBlockhash` call is made first and the result is shared across
/// all items in the batch. Items whose prerequisite fetch fails are
/// re-enqueued for the next run; items that do not need a slot/blockhash are
/// silently dropped from this batch and will be re-enqueued by their
/// respective scheduler timers on their next firing.
pub async fn execute_rpc_queue<R: CanisterRuntime>(runtime: R) {
    let _guard = match TimerGuard::new(TaskType::ExecuteRpcQueue) {
        Ok(g) => g,
        Err(_) => return,
    };

    let items = dequeue_batch(MAX_CONCURRENT_RPC_CALLS);
    if items.is_empty() {
        return;
    }

    let needs_slot_and_blockhash = items.iter().any(WorkItem::needs_slot_and_blockhash);
    let slot_and_blockhash = if needs_slot_and_blockhash {
        match get_recent_slot_and_blockhash(&runtime).await {
            Ok(result) => Some(result),
            Err(e) => {
                log!(
                    Priority::Info,
                    "Executor: failed to fetch slot and blockhash: {e}"
                );
                // Re-enqueue only items that needed slot/blockhash so they are
                // retried on the next executor run.
                for item in items.into_iter().filter(WorkItem::needs_slot_and_blockhash) {
                    enqueue(item);
                }
                return;
            }
        }
    } else {
        None
    };

    futures::future::join_all(
        items
            .into_iter()
            .map(|item| execute_work_item(&runtime, item, slot_and_blockhash)),
    )
    .await;

    if !queue_is_empty() {
        runtime.set_timer(Duration::ZERO, execute_rpc_queue);
    }
}

async fn execute_work_item<R: CanisterRuntime>(
    runtime: &R,
    item: WorkItem,
    slot_and_blockhash: Option<(Slot, Hash)>,
) {
    match item {
        WorkItem::CheckSignatureStatuses {
            signatures,
            submitted_slots,
        } => {
            let (current_slot, _) =
                slot_and_blockhash.expect("BUG: slot required for CheckSignatureStatuses");
            execute_check_signature_statuses(runtime, signatures, submitted_slots, current_slot)
                .await;
        }
        WorkItem::PollMonitoredAddress(account) => {
            execute_poll_monitored_address(runtime, account).await;
        }
        WorkItem::SubmitConsolidationBatch(funds) => {
            let (slot, blockhash) = slot_and_blockhash
                .expect("BUG: slot+blockhash required for SubmitConsolidationBatch");
            execute_submit_consolidation_batch(runtime, funds, slot, blockhash).await;
        }
        WorkItem::SubmitWithdrawalBatch(requests) => {
            let (slot, blockhash) =
                slot_and_blockhash.expect("BUG: slot+blockhash required for SubmitWithdrawalBatch");
            execute_submit_withdrawal_batch(runtime, requests, slot, blockhash).await;
        }
        WorkItem::ResubmitTransaction {
            old_signature,
            message,
            signers,
        } => {
            let (new_slot, new_blockhash) =
                slot_and_blockhash.expect("BUG: slot+blockhash required for ResubmitTransaction");
            execute_resubmit_transaction(
                runtime,
                old_signature,
                message,
                signers,
                new_slot,
                new_blockhash,
            )
            .await;
        }
    }
}

// ---------------------------------------------------------------------------
// Execution logic
// ---------------------------------------------------------------------------

async fn execute_check_signature_statuses<R: CanisterRuntime>(
    runtime: &R,
    signatures: Vec<Signature>,
    submitted_slots: BTreeMap<Signature, Slot>,
    current_slot: Slot,
) {
    match get_signature_statuses(runtime, &signatures).await {
        Err(e) => {
            log!(Priority::Info, "Failed to check transaction statuses: {e}");
        }
        Ok(statuses) => {
            for (signature, status) in signatures.iter().zip(statuses) {
                match status {
                    Some(s)
                        if s.confirmation_status
                            == Some(TransactionConfirmationStatus::Finalized) =>
                    {
                        if let Some(err) = s.err {
                            log!(
                                Priority::Error,
                                "Transaction {signature} finalized with on-chain error: {err:?}"
                            );
                            mutate_state(|state| {
                                process_event(
                                    state,
                                    EventType::FailedTransaction {
                                        signature: *signature,
                                    },
                                    runtime,
                                )
                            });
                        } else {
                            log!(Priority::Info, "Transaction {signature} finalized");
                            mutate_state(|state| {
                                process_event(
                                    state,
                                    EventType::SucceededTransaction {
                                        signature: *signature,
                                    },
                                    runtime,
                                )
                            });
                        }
                    }
                    Some(_) => {} // in-flight (Processed/Confirmed)
                    None => {
                        if let Some(&submitted_slot) = submitted_slots.get(signature) {
                            if submitted_slot + MAX_BLOCKHASH_AGE < current_slot {
                                log!(
                                    Priority::Info,
                                    "Transaction {signature} expired, marking for resubmission"
                                );
                                mutate_state(|state| {
                                    process_event(
                                        state,
                                        EventType::ExpiredTransaction {
                                            signature: *signature,
                                        },
                                        runtime,
                                    )
                                });
                            }
                        }
                    }
                }
            }
        }
    }
}

async fn execute_poll_monitored_address<R: CanisterRuntime>(runtime: &R, account: Account) {
    let master_key = lazy_get_schnorr_master_key(runtime).await;
    let deposit_address = account_address(&master_key, &account);

    let params = GetSignaturesForAddressParams {
        pubkey: deposit_address.into(),
        commitment: Some(CommitmentLevel::Finalized),
        min_context_slot: None,
        limit: Some(
            (MAX_TRANSACTIONS_PER_ACCOUNT as u32)
                .try_into()
                .expect("MAX_TRANSACTIONS_PER_ACCOUNT must be between 1 and 1000"),
        ),
        before: None,
        until: None,
    };

    match get_signatures_for_address(runtime, params).await {
        Err(e) => {
            log!(
                Priority::Info,
                "Failed to get signatures for address {deposit_address}: {e}"
            );
        }
        Ok(signatures) => {
            let new_sigs: Vec<Signature> = signatures
                .into_iter()
                .filter(|s| s.err.is_none())
                .map(|s| s.signature.into())
                .collect();
            if !new_sigs.is_empty() {
                PENDING_SIGNATURES.with(|pending| {
                    pending
                        .borrow_mut()
                        .entry(account)
                        .or_default()
                        .extend(new_sigs);
                });
            }
        }
    }

    mutate_state(|state| {
        process_event(
            state,
            EventType::StoppedMonitoringAccount { account },
            runtime,
        );
    });
}

async fn execute_submit_consolidation_batch<R: CanisterRuntime>(
    runtime: &R,
    funds_to_consolidate: Vec<(Account, (Lamport, Vec<LedgerMintIndex>))>,
    slot: Slot,
    recent_blockhash: Hash,
) {
    let (sources, mint_indices): (Vec<_>, Vec<_>) = funds_to_consolidate
        .into_iter()
        .map(|(account, (lamport, indices))| ((account, lamport), indices))
        .unzip();

    let (transaction, signers) =
        match create_signed_consolidation_transaction(runtime, sources, recent_blockhash).await {
            Ok(tx) => tx,
            Err(e) => {
                log!(
                    Priority::Error,
                    "Failed to create deposit consolidation transaction: {e}"
                );
                return;
            }
        };

    let signature = transaction.signatures[0];
    let message = transaction.message.clone();

    mutate_state(|state| {
        process_event(
            state,
            EventType::SubmittedTransaction {
                signature,
                message: message.into(),
                signers,
                slot,
                purpose: TransactionPurpose::ConsolidateDeposits {
                    mint_indices: mint_indices.into_iter().flatten().collect(),
                },
            },
            runtime,
        )
    });

    match submit_transaction(runtime, transaction).await {
        Ok(_) => log!(
            Priority::Info,
            "Submitted consolidation transaction {signature}"
        ),
        Err(e) => log!(
            Priority::Info,
            "Failed to submit deposit consolidation transaction (will retry): {e}"
        ),
    }
}

async fn execute_submit_withdrawal_batch<R: CanisterRuntime>(
    runtime: &R,
    requests: Vec<WithdrawalRequest>,
    slot: Slot,
    recent_blockhash: Hash,
) {
    let targets: Vec<_> = requests
        .iter()
        .map(|r| (Address::from(r.solana_address), r.amount_to_transfer))
        .collect();

    let (signed_tx, signers) = match create_signed_batch_withdrawal_transaction(
        runtime,
        &targets,
        recent_blockhash,
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
                slot,
                purpose: TransactionPurpose::WithdrawSol {
                    burn_indices: burn_indices.clone(),
                },
            },
            runtime,
        )
    });

    match submit_transaction(runtime, signed_tx).await {
        Ok(_) => log!(
            Priority::Info,
            "Submitted withdrawal transaction {signature} for burn indices {burn_indices:?}"
        ),
        Err(e) => log!(
            Priority::Info,
            "Failed to send withdrawal transaction {signature} (will be resubmitted): {e}"
        ),
    }
}

async fn execute_resubmit_transaction<R: CanisterRuntime>(
    runtime: &R,
    old_signature: Signature,
    versioned_message: VersionedMessage,
    signers: Vec<Account>,
    new_slot: Slot,
    new_blockhash: Hash,
) {
    let VersionedMessage::Legacy(mut message) = versioned_message;
    message.recent_blockhash = new_blockhash;

    let mut transaction = Transaction::new_unsigned(message);
    transaction.signatures = match sign_bytes(
        signers.iter().map(derivation_path),
        &runtime.signer(),
        transaction.message_data(),
    )
    .await
    {
        Ok(sigs) => sigs,
        Err(e) => {
            log!(
                Priority::Info,
                "Failed to sign resubmission of {old_signature}: {e}"
            );
            return;
        }
    };

    let new_signature = transaction.signatures[0];

    mutate_state(|state| {
        process_event(
            state,
            EventType::ResubmittedTransaction {
                old_signature,
                new_signature,
                new_slot,
            },
            runtime,
        )
    });

    match submit_transaction(runtime, transaction).await {
        Ok(_) => log!(
            Priority::Info,
            "Resubmitted transaction {old_signature} as {new_signature}"
        ),
        Err(e) => log!(
            Priority::Info,
            "Failed to resubmit transaction {old_signature}: {e}"
        ),
    }
}

// ---------------------------------------------------------------------------
// Test / bench helpers
// ---------------------------------------------------------------------------

#[cfg(any(test, feature = "canbench-rs"))]
pub fn pending_signatures_for(account: &Account) -> Vec<Signature> {
    PENDING_SIGNATURES.with(|p| {
        p.borrow()
            .get(account)
            .map(|q| q.iter().copied().collect())
            .unwrap_or_default()
    })
}

#[cfg(any(test, feature = "canbench-rs"))]
pub fn reset_pending_signatures() {
    PENDING_SIGNATURES.with(|p| p.borrow_mut().clear());
}

#[cfg(any(test, feature = "canbench-rs"))]
pub fn reset_work_queue() {
    WORK_QUEUE.with(|q| q.borrow_mut().clear());
}
