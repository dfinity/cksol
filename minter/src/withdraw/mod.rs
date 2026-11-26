use std::str::FromStr;
use std::time::Duration;

use cksol_types::{BurnMemo, Memo, WithdrawSolError, WithdrawSolOk, WithdrawSolStatus};
use icrc_ledger_types::icrc1::account::Account;
use sol_rpc_types::Lamport;
use solana_address::Address;
use solana_hash::Hash;
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
        CreatedWithdrawalTransaction, State, TaskType, WithdrawalPlan,
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
pub mod spl;
#[cfg(test)]
mod tests;

pub async fn withdraw<R: CanisterRuntime>(
    runtime: &R,
    from: Account,
    amount_to_burn: u64,
    address: String,
) -> Result<WithdrawSolOk, WithdrawSolError> {
    let solana_address = validate_withdrawal_request(amount_to_burn, &address)?;
    validate_nonce_pool_not_empty()?;

    let _guard = withdrawal_guard(from)?;

    let minter_account: Account = runtime.canister_self().into();
    let block_index = burn(
        runtime,
        minter_account,
        from,
        amount_to_burn,
        Memo::from(BurnMemo::convert(solana_address)),
    )
    .await
    .map_err(|e| match e {
        BurnError::TemporarilyUnavailable(msg) => WithdrawSolError::TemporarilyUnavailable(msg),
        BurnError::InsufficientFunds { balance } => WithdrawSolError::InsufficientFunds { balance },
        BurnError::InsufficientAllowance { allowance } => {
            WithdrawSolError::InsufficientAllowance { allowance }
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

    Ok(WithdrawSolOk { block_index })
}

/// Checks the amount and the destination of a withdrawal request and returns the parsed
/// destination. Also used to describe the request in an ICRC-21 consent message.
pub(crate) fn validate_withdrawal_request(
    amount: Lamport,
    address: &str,
) -> Result<Address, WithdrawSolError> {
    let minimum_withdrawal_amount = read_state(|s| s.minimum_withdrawal_amount());
    if amount < minimum_withdrawal_amount {
        return Err(WithdrawSolError::ValueTooSmall {
            minimum_withdrawal_amount,
            withdrawal_amount: amount,
        });
    }

    let solana_address = Address::from_str(address)
        .map_err(|e| WithdrawSolError::MalformedAddress(e.to_string()))?;
    validate_destination(&solana_address)?;
    Ok(solana_address)
}

enum DestinationError {
    InvalidDestination(String),
    TemporarilyUnavailable(String),
}

impl From<DestinationError> for WithdrawSolError {
    fn from(error: DestinationError) -> Self {
        match error {
            DestinationError::InvalidDestination(message) => Self::InvalidDestination(message),
            DestinationError::TemporarilyUnavailable(message) => {
                Self::TemporarilyUnavailable(message)
            }
        }
    }
}

fn validate_destination(destination: &Address) -> Result<(), DestinationError> {
    if reserved_account_keys::is_reserved_account_key(destination) {
        return Err(DestinationError::InvalidDestination(format!(
            "{destination} is an account key reserved by the Solana runtime"
        )));
    }
    if read_state(|s| s.nonce_pool().contains(destination)) {
        return Err(DestinationError::InvalidDestination(format!(
            "{destination} is a durable nonce account of the ckSOL minter"
        )));
    }
    let master_key =
        minter_public_key().map_err(|e| DestinationError::TemporarilyUnavailable(e.to_string()))?;
    if destination == &minter_address(&master_key) {
        return Err(DestinationError::InvalidDestination(format!(
            "{destination} is the ckSOL minter's main address"
        )));
    }
    Ok(())
}

fn validate_nonce_pool_not_empty() -> Result<(), DestinationError> {
    if read_state(|s| s.nonce_pool().is_empty()) {
        return Err(DestinationError::TemporarilyUnavailable(
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

struct ReservedPlan {
    nonce_account: Address,
    plan: WithdrawalPlan,
}

/// A created transaction, rebuilt from state for signing.
pub(super) struct BoundWithdrawal {
    pub(super) nonce_account: Address,
    pub(super) purpose: TransactionPurpose,
    pub(super) message: Message,
}

async fn create_transactions_batch<R: CanisterRuntime>(
    runtime: &R,
    minter_address: Address,
) -> usize {
    let reserved_plans: Vec<ReservedPlan> = read_state(|state| {
        let max_plans = MAX_CONCURRENT_RPC_CALLS.min(
            MAX_CONCURRENT_SIGNATURES.saturating_sub(state.num_unsigned_withdrawal_transactions()),
        );
        state
            .nonce_pool()
            .free_accounts()
            .copied()
            .zip(state.withdrawal_plans())
            .take(max_plans)
            .map(|(nonce_account, plan)| ReservedPlan {
                nonce_account,
                plan,
            })
            .collect()
    });

    futures::future::join_all(
        reserved_plans
            .into_iter()
            .map(async |plan| create_transaction(runtime, minter_address, plan).await),
    )
    .await
    .into_iter()
    .filter(|created| *created)
    .count()
}

async fn create_transaction<R: CanisterRuntime>(
    runtime: &R,
    minter_address: Address,
    reserved: ReservedPlan,
) -> bool {
    let ReservedPlan {
        nonce_account,
        plan,
    } = reserved;
    let label = match &plan {
        WithdrawalPlan::Sol(requests) => format!(
            "withdrawal batch for burn indices {:?}",
            requests
                .iter()
                .map(|request| request.burn_block_index)
                .collect::<Vec<_>>()
        ),
        WithdrawalPlan::Spl(request) => format!(
            "SPL withdrawal for burn index {}",
            request.cksol_burn_block_index
        ),
    };
    let nonce_value = match read_verified_nonce(runtime, nonce_account, minter_address).await {
        Ok(nonce_value) => nonce_value,
        Err(e) => {
            log!(
                Priority::Error,
                "Failed to read nonce account {nonce_account}, skipping {label} this round: {e}"
            );
            return false;
        }
    };
    if read_state(|state| state.nonce_pool().has_seen(&nonce_account, &nonce_value)) {
        log!(
            Priority::Info,
            "Read a stale nonce value for account {nonce_account}, skipping {label} this round"
        );
        return false;
    }
    match plan {
        WithdrawalPlan::Sol(requests) => create_sol_transaction(
            runtime,
            &minter_address,
            nonce_account,
            nonce_value,
            requests,
        ),
        WithdrawalPlan::Spl(request) => spl::create_spl_transaction(
            runtime,
            &minter_address,
            nonce_account,
            nonce_value,
            &request,
        ),
    }
}

/// Builds the batch message for the read nonce, re-checks the balance, and records the binding.
fn create_sol_transaction<R: CanisterRuntime>(
    runtime: &R,
    minter_address: &Address,
    nonce_account: Address,
    nonce_value: Hash,
    requests: Vec<WithdrawalRequest>,
) -> bool {
    let burn_indices: Vec<_> = requests.iter().map(|r| r.burn_block_index).collect();
    // An SPL sweep may have reserved part of the balance during the nonce read.
    if !read_state(|state| state.covers_withdrawal_batch(&requests)) {
        log!(
            Priority::Info,
            "The balance no longer covers the withdrawal batch for burn indices {burn_indices:?}, skipping it this round"
        );
        return false;
    }
    if let Err(e) = build_batch_withdrawal_message(
        minter_address,
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

/// The created transactions of both kinds, oldest ckSOL burn index first.
fn bound_withdrawals(state: &State, minter_address: &Address) -> Vec<BoundWithdrawal> {
    let sol = state
        .created_withdrawal_txs()
        .iter()
        .map(|(nonce_account, created)| {
            (
                created.burn_indices.iter().min().copied(),
                nonce_account,
                bound_withdrawal(state, minter_address, nonce_account, created),
            )
        });
    let spl = state
        .created_spl_withdrawal_txs()
        .iter()
        .map(|(nonce_account, created)| {
            (
                Some(created.cksol_burn_block_index),
                nonce_account,
                spl::bound_withdrawal(state, nonce_account, created),
            )
        });
    let mut bound: Vec<_> = sol.collect();
    // A halt holds the created SPL transactions with their nonce bindings. A transaction
    // whose signing began before the halt is in flight, so the round still sends it.
    if !state.spl_withdrawals_halted() {
        bound.extend(spl);
    }
    bound.sort_by_key(|(oldest_burn_index, _, _)| *oldest_burn_index);
    bound
        .into_iter()
        .take(MAX_CONCURRENT_SIGNATURES)
        .filter_map(|(_, nonce_account, bound)| {
            bound
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
        purpose: TransactionPurpose::Withdrawal {
            burn_indices: created.burn_indices.clone(),
        },
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
        purpose,
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
                purpose,
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
