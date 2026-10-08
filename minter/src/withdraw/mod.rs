use std::str::FromStr;
use std::time::Duration;

use cksol_types::{WithdrawSolStatus, WithdrawalError, WithdrawalOk};
use icrc_ledger_types::icrc1::account::Account;
use sol_rpc_types::Lamport;
use solana_address::Address;
use solana_message::Message;
use solana_transaction::Transaction;

use canlog::log;
use cksol_types_internal::log::Priority;

use crate::{
    address::{minter_address, minter_public_key},
    constants::{MAX_CONCURRENT_RPC_CALLS, MAX_CONCURRENT_SIGNATURES},
    guard::{TimerGuard, withdrawal_guard},
    ledger::{BurnError, burn},
    numeric::LedgerBurnIndex,
    rpc::submit_transaction_skipping_preflight,
    runtime::CanisterRuntime,
    sol_transfer::{
        CreateTransferError, build_batch_withdrawal_message, sign_batch_withdrawal_message,
    },
    state::{
        CreatedWithdrawalTransaction, State, TaskType,
        audit::process_event,
        event::{EventType, Signer, TransactionPurpose, WithdrawalRequest},
        mutate_state, read_state,
    },
    withdraw::nonce::read_verified_nonce,
};

pub const WITHDRAWAL_PROCESSING_DELAY: Duration = Duration::from_mins(1);
pub const WITHDRAWAL_PROCESSING_RETRY_DELAY: Duration = Duration::from_secs(10);

const _: () = assert!(MAX_CONCURRENT_SIGNATURES <= MAX_CONCURRENT_RPC_CALLS);

pub mod nonce;
mod reserved_account_keys;
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

    let solana_address = Address::from_str(&address)
        .map_err(|e| WithdrawalError::MalformedAddress(e.to_string()))?;
    validate_destination(&solana_address)?;
    validate_nonce_pool_not_empty()?;

    let _guard = withdrawal_guard(from)?;

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

fn validate_destination(destination: &Address) -> Result<(), WithdrawalError> {
    if reserved_account_keys::is_reserved_account_key(destination) {
        return Err(WithdrawalError::InvalidDestination(format!(
            "{destination} is an account key reserved by the Solana runtime"
        )));
    }
    if read_state(|s| s.nonce_pool().contains(destination)) {
        return Err(WithdrawalError::InvalidDestination(format!(
            "{destination} is a durable nonce account of the ckSOL minter"
        )));
    }
    let master_key =
        minter_public_key().map_err(|e| WithdrawalError::TemporarilyUnavailable(e.to_string()))?;
    if destination == &minter_address(&master_key) {
        return Err(WithdrawalError::InvalidDestination(format!(
            "{destination} is the ckSOL minter's main address"
        )));
    }
    Ok(())
}

fn validate_nonce_pool_not_empty() -> Result<(), WithdrawalError> {
    if read_state(|s| s.nonce_pool().is_empty()) {
        return Err(WithdrawalError::TemporarilyUnavailable(
            "The durable nonce account pool is empty, no withdrawal can be processed".to_string(),
        ));
    }
    Ok(())
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

    let Some(minter_address) = read_state(|s| s.minter_public_key().map(minter_address)) else {
        log!(
            Priority::Info,
            "Minter public key is not yet available, skipping withdrawal processing"
        );
        return;
    };

    let num_created = create_transactions_batch(&runtime, minter_address).await;
    let signed_transactions = sign_transactions_batch(&runtime, minter_address).await;
    let made_progress = num_created > 0 || !signed_transactions.is_empty();
    send_transactions_batch(&runtime, signed_transactions).await;

    if made_progress
        && read_state(|s| {
            s.can_create_withdrawal_transaction() || s.has_unsigned_withdrawal_transaction()
        })
    {
        runtime.set_timer(
            WITHDRAWAL_PROCESSING_RETRY_DELAY,
            process_pending_withdrawals,
        );
    }
}

struct ReservedBatch {
    nonce_account: Address,
    requests: Vec<WithdrawalRequest>,
}

struct BoundWithdrawal {
    nonce_account: Address,
    burn_indices: Vec<LedgerBurnIndex>,
    message: Message,
}

async fn create_transactions_batch<R: CanisterRuntime>(
    runtime: &R,
    minter_address: Address,
) -> usize {
    let reserved_batches: Vec<ReservedBatch> = read_state(|state| {
        let max_batches = MAX_CONCURRENT_RPC_CALLS
            .min(MAX_CONCURRENT_SIGNATURES.saturating_sub(state.created_withdrawal_txs().len()));
        state
            .nonce_pool()
            .free_accounts()
            .copied()
            .zip(state.withdrawal_batches())
            .take(max_batches)
            .map(|(nonce_account, requests)| ReservedBatch {
                nonce_account,
                requests,
            })
            .collect()
    });

    futures::future::join_all(
        reserved_batches
            .into_iter()
            .map(async |batch| create_transaction(runtime, minter_address, batch).await),
    )
    .await
    .into_iter()
    .filter(|created| *created)
    .count()
}

async fn create_transaction<R: CanisterRuntime>(
    runtime: &R,
    minter_address: Address,
    batch: ReservedBatch,
) -> bool {
    let ReservedBatch {
        nonce_account,
        requests,
    } = batch;
    let nonce_value = match read_verified_nonce(runtime, nonce_account, minter_address).await {
        Ok(nonce_value) => nonce_value,
        Err(e) => {
            log!(
                Priority::Error,
                "Failed to read nonce account {nonce_account}, skipping withdrawal batch this round: {e}"
            );
            return false;
        }
    };
    if read_state(|state| state.nonce_pool().has_seen(&nonce_account, &nonce_value)) {
        log!(
            Priority::Info,
            "Read a stale nonce value for account {nonce_account}, skipping withdrawal batch this round"
        );
        return false;
    }

    let burn_indices: Vec<_> = requests.iter().map(|r| r.burn_block_index).collect();
    if let Err(e) = build_batch_withdrawal_message(
        &minter_address,
        &nonce_account,
        nonce_value,
        &withdrawal_transfers(&requests),
    ) {
        log!(
            Priority::Error,
            "Failed to build batch withdrawal transaction for burn indices {burn_indices:?}: {e}"
        );
        return false;
    }

    mutate_state(|state| {
        process_event(
            state,
            EventType::CreatedWithdrawalTransaction {
                burn_indices,
                nonce_account,
                nonce_value,
            },
            runtime,
        )
    });
    true
}

async fn sign_transactions_batch<R: CanisterRuntime>(
    runtime: &R,
    minter_address: Address,
) -> Vec<Transaction> {
    let bound_withdrawals = read_state(|state| bound_withdrawals(state, &minter_address));
    futures::future::join_all(
        bound_withdrawals
            .into_iter()
            .map(async |withdrawal| sign_transaction(runtime, withdrawal).await),
    )
    .await
    .into_iter()
    .flatten()
    .collect()
}

fn bound_withdrawals(state: &State, minter_address: &Address) -> Vec<BoundWithdrawal> {
    let mut created_withdrawal_txs: Vec<_> = state.created_withdrawal_txs().iter().collect();
    created_withdrawal_txs.sort_by_key(|(_, created)| created.burn_indices.iter().min().copied());
    created_withdrawal_txs
        .into_iter()
        .take(MAX_CONCURRENT_SIGNATURES)
        .filter_map(|(nonce_account, created)| {
            bound_withdrawal(state, minter_address, nonce_account, created)
                .inspect_err(|e| {
                    log!(
                        Priority::Error,
                        "Failed to rebuild withdrawal transaction bound to nonce account {nonce_account}: {e}"
                    )
                })
                .ok()
        })
        .collect()
}

fn bound_withdrawal(
    state: &State,
    minter_address: &Address,
    nonce_account: &Address,
    created: &CreatedWithdrawalTransaction,
) -> Result<BoundWithdrawal, CreateTransferError> {
    let requests: Vec<WithdrawalRequest> = created
        .burn_indices
        .iter()
        .map(|burn_index| created_withdrawal_request(state, burn_index))
        .collect();
    let message = build_batch_withdrawal_message(
        minter_address,
        nonce_account,
        created.nonce_value,
        &withdrawal_transfers(&requests),
    )?;
    Ok(BoundWithdrawal {
        nonce_account: *nonce_account,
        burn_indices: created.burn_indices.clone(),
        message,
    })
}

fn created_withdrawal_request(state: &State, burn_index: &LedgerBurnIndex) -> WithdrawalRequest {
    state
        .created_withdrawal_requests()
        .get(burn_index)
        .unwrap_or_else(|| {
            panic!("BUG: withdrawal request {burn_index:?} of a created transaction is not in the created bucket")
        })
        .request
        .clone()
}

fn withdrawal_transfers(requests: &[WithdrawalRequest]) -> Vec<(Address, Lamport)> {
    requests
        .iter()
        .map(|request| {
            (
                Address::from(request.solana_address),
                request.amount_to_transfer,
            )
        })
        .collect()
}

async fn sign_transaction<R: CanisterRuntime>(
    runtime: &R,
    withdrawal: BoundWithdrawal,
) -> Option<Transaction> {
    let BoundWithdrawal {
        nonce_account,
        burn_indices,
        message,
    } = withdrawal;
    let transaction = match sign_batch_withdrawal_message(runtime, message).await {
        Ok(transaction) => transaction,
        Err(e) => {
            log!(
                Priority::Error,
                "Failed to sign withdrawal transaction bound to nonce account {nonce_account} (will be re-signed next round): {e}"
            );
            return None;
        }
    };
    mutate_state(|state| {
        process_event(
            state,
            EventType::SubmittedTransaction {
                signature: transaction.signatures[0],
                message: transaction.message.clone().into(),
                signers: vec![Signer::Minter],
                purpose: TransactionPurpose::Withdrawal { burn_indices },
            },
            runtime,
        )
    });
    Some(transaction)
}

async fn send_transactions_batch<R: CanisterRuntime>(runtime: &R, transactions: Vec<Transaction>) {
    futures::future::join_all(
        transactions
            .into_iter()
            .map(async |transaction| send_transaction(runtime, transaction).await),
    )
    .await;
}

async fn send_transaction<R: CanisterRuntime>(runtime: &R, transaction: Transaction) {
    let signature = transaction.signatures[0];
    match submit_transaction_skipping_preflight(runtime, transaction).await {
        Ok(_) => {
            log!(
                Priority::Info,
                "Submitted withdrawal transaction {signature}"
            );
        }
        Err(e) => {
            log!(
                Priority::Info,
                "Failed to send withdrawal transaction {signature}: {e}"
            );
        }
    }
}

pub fn withdrawal_status(block_index: u64) -> WithdrawSolStatus {
    read_state(|s| s.withdrawal_status(block_index))
}
