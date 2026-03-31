use crate::{
    address::{derivation_path, derive_public_key, lazy_get_schnorr_master_key},
    constants::MAX_CONCURRENT_RPC_CALLS,
    guard::TimerGuard,
    numeric::LedgerMintIndex,
    runtime::CanisterRuntime,
    sol_transfer::{
        CreateTransferError, MAX_SIGNATURES, create_signed_batch_consolidation_transaction,
    },
    state::{
        TaskType,
        audit::process_event,
        event::{EventType, TransactionPurpose},
        mutate_state, read_state,
    },
    transaction::{SubmitTransactionError, get_recent_slot_and_blockhash, submit_transaction},
};
use canlog::log;
use cksol_types_internal::log::Priority;
use icrc_ledger_types::icrc1::account::Account;
use itertools::Itertools;
use sol_rpc_types::{Lamport, Slot};
use solana_address::Address;
use solana_hash::Hash;
use solana_signature::Signature;
use std::collections::BTreeMap;
use std::time::Duration;
use thiserror::Error;

#[cfg(test)]
mod tests;

pub const DEPOSIT_CONSOLIDATION_DELAY: Duration = Duration::from_mins(10);

pub(crate) const MAX_TRANSFERS_PER_CONSOLIDATION: usize = MAX_SIGNATURES as usize - 1;

pub async fn consolidate_deposits<R: CanisterRuntime>(runtime: R) {
    let _guard = match TimerGuard::new(TaskType::DepositConsolidation) {
        Ok(guard) => guard,
        Err(_) => return,
    };

    let consolidation_rounds: Vec<Vec<_>> =
        read_state(|s| group_deposits_by_account(s.deposits_to_consolidate()))
            .into_iter()
            .chunks(MAX_TRANSFERS_PER_CONSOLIDATION)
            .into_iter()
            .map(Iterator::collect)
            .collect();

    for round in &consolidation_rounds
        .into_iter()
        .chunks(MAX_CONCURRENT_RPC_CALLS)
    {
        let (slot, recent_blockhash) = match get_recent_slot_and_blockhash(&runtime).await {
            Ok((slot, blockhash)) => (slot, blockhash),
            Err(e) => {
                log!(Priority::Info, "Failed to fetch recent blockhash: {e}");
                return;
            }
        };

        futures::future::join_all(round.map(async |funds| {
            match submit_consolidation_transaction(&runtime, funds, slot, recent_blockhash).await {
                Ok(sig) => log!(Priority::Info, "Submitted consolidation transaction {sig}"),
                Err(e) => log!(Priority::Info, "Deposit consolidation failed: {e}"),
            }
        }))
        .await;
    }
}

fn group_deposits_by_account(
    deposits: &BTreeMap<LedgerMintIndex, (Account, Lamport)>,
) -> Vec<(Account, (Lamport, Vec<LedgerMintIndex>))> {
    let mut by_account: BTreeMap<Account, (Lamport, Vec<LedgerMintIndex>)> = BTreeMap::new();
    for (mint_index, (account, lamport)) in deposits {
        let entry = by_account.entry(*account).or_default();
        entry.0 += lamport;
        entry.1.push(*mint_index);
    }
    by_account.into_iter().collect()
}

#[derive(Debug, Error)]
enum ConsolidationError {
    #[error("failed to create transaction: {0}")]
    CreateTransactionFailed(#[from] CreateTransferError),
    #[error("failed to submit transaction: {0}")]
    SubmitTransactionFailed(#[from] SubmitTransactionError),
}

async fn submit_consolidation_transaction<R: CanisterRuntime>(
    runtime: &R,
    funds_to_consolidate: Vec<(Account, (Lamport, Vec<LedgerMintIndex>))>,
    slot: Slot,
    recent_blockhash: Hash,
) -> Result<Signature, ConsolidationError> {
    let minter_account = Account {
        owner: runtime.canister_self(),
        subaccount: None,
    };
    let master_key = lazy_get_schnorr_master_key().await;
    let minter_address = Address::from(
        derive_public_key(&master_key, derivation_path(&minter_account)).serialize_raw(),
    );

    let sources: Vec<(Account, Lamport)> = funds_to_consolidate
        .iter()
        .map(|(account, (lamport, _))| (*account, *lamport))
        .collect();
    let (transaction, signers) = create_signed_batch_consolidation_transaction(
        minter_account,
        &sources,
        minter_address,
        recent_blockhash,
        &runtime.signer(),
    )
    .await?;

    let signature = transaction.signatures[0];
    let message = transaction.message.clone();
    let mint_indices = funds_to_consolidate
        .into_iter()
        .flat_map(|(_, (_, indices))| indices)
        .collect();

    mutate_state(|state| {
        process_event(
            state,
            EventType::SubmittedTransaction {
                signature,
                message: message.into(),
                signers,
                slot,
                purpose: TransactionPurpose::ConsolidateDeposits { mint_indices },
            },
            runtime,
        )
    });

    submit_transaction(runtime, transaction).await?;

    Ok(signature)
}
