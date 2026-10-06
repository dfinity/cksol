use crate::{
    constants::{LEDGER_DEDUPLICATION_WINDOW, MAX_PENDING_MINTS_PER_ROUND},
    guard::TimerGuard,
    ledger::client::to_ledger_mint_index,
    numeric::LedgerMintIndex,
    runtime::CanisterRuntime,
    state::{
        PendingMint, State, TaskType, audit::process_event, event::EventType, mutate_state,
        read_state,
    },
    storage::{FailedMintReason, record_failed_mint_attempt},
};
use canlog::log;
use cksol_types::{DepositSolId, Memo, MintMemo};
use cksol_types_internal::log::Priority;
use icrc_ledger_types::icrc1::transfer::{NumTokens, TransferArg, TransferError};
use scopeguard::ScopeGuard;
use std::time::Duration;

#[cfg(test)]
mod tests;

pub async fn process_pending_mints<R: CanisterRuntime>(runtime: R) {
    let _guard = match TimerGuard::new(TaskType::Mint) {
        Ok(guard) => guard,
        Err(_) => return,
    };

    let MintRound {
        pending_mints,
        more_pending_mints_than_one_round,
    } = read_state(MintRound::next);

    if pending_mints.is_empty() {
        return;
    }

    let selected_deposit_ids: Vec<DepositSolId> = pending_mints
        .iter()
        .map(|(deposit_id, _)| *deposit_id)
        .collect();

    for (deposit_id, pending) in pending_mints {
        process_pending_mint(&runtime, deposit_id, pending).await;
    }

    let round_made_progress = read_state(|state| {
        selected_deposit_ids
            .iter()
            .any(|deposit_id| !state.deposits().pending_mints().contains_key(deposit_id))
    });
    if round_made_progress && more_pending_mints_than_one_round {
        runtime.set_timer(Duration::ZERO, process_pending_mints);
    }
}

struct MintRound {
    pending_mints: Vec<(DepositSolId, PendingMint)>,
    more_pending_mints_than_one_round: bool,
}

impl MintRound {
    fn next(state: &State) -> Self {
        Self {
            pending_mints: state
                .deposits()
                .pending_mints()
                .iter()
                .map(|(deposit_id, pending)| (*deposit_id, *pending))
                .take(MAX_PENDING_MINTS_PER_ROUND)
                .collect(),
            more_pending_mints_than_one_round: state.deposits().pending_mints().len()
                > MAX_PENDING_MINTS_PER_ROUND,
        }
    }
}

async fn process_pending_mint<R: CanisterRuntime>(
    runtime: &R,
    deposit_id: DepositSolId,
    pending: PendingMint,
) {
    if is_beyond_deduplication_window(&pending, runtime.time()) {
        log!(
            Priority::Error,
            "Quarantining deposit {deposit_id}: its pending mint is older than the deduplication window of the ledger"
        );
        record_failed_mint_attempt(FailedMintReason::Expired);
        record_quarantined_pending_mint(runtime, deposit_id);
        return;
    }

    let quarantine_unless_defused = scopeguard::guard(deposit_id, |deposit_id| {
        record_quarantined_pending_mint(runtime, deposit_id);
    });

    let client = read_state(|state| state.ledger_client(runtime.inter_canister_call_runtime()));
    match client
        .transfer(mint_transfer_arg(deposit_id, &pending))
        .await
    {
        Ok(Ok(mint_block_index)) => {
            record_minted_swept_deposit(runtime, deposit_id, mint_block_index);
            log!(
                Priority::Info,
                "Minted {} lamports for deposit {deposit_id} (ledger block index {})",
                pending.amount_to_mint,
                mint_block_index.get()
            );
            ScopeGuard::into_inner(quarantine_unless_defused);
        }
        Ok(Err(TransferError::Duplicate { duplicate_of })) => {
            let mint_block_index = to_ledger_mint_index(duplicate_of);
            record_minted_swept_deposit(runtime, deposit_id, mint_block_index);
            log!(
                Priority::Info,
                "Mint of deposit {deposit_id} was deduplicated by the ledger (ledger block index {})",
                mint_block_index.get()
            );
            ScopeGuard::into_inner(quarantine_unless_defused);
        }
        Ok(Err(TransferError::TooOld)) => {
            record_failed_mint_attempt(FailedMintReason::Expired);
            log!(
                Priority::Error,
                "Quarantining deposit {deposit_id}: the ledger rejected its mint as outside the deduplication window"
            );
        }
        Ok(Err(
            rejection @ (TransferError::BadFee { .. }
            | TransferError::BadBurn { .. }
            | TransferError::InsufficientFunds { .. }),
        )) => {
            record_failed_mint_attempt(FailedMintReason::Rejected);
            log!(
                Priority::Error,
                "Quarantining deposit {deposit_id}: the ledger definitively rejected its mint from the minting account: {rejection:?}"
            );
        }
        Ok(Err(
            transient @ (TransferError::TemporarilyUnavailable
            | TransferError::GenericError { .. }
            | TransferError::CreatedInFuture { .. }),
        )) => {
            ScopeGuard::into_inner(quarantine_unless_defused);
            record_failed_mint_attempt(FailedMintReason::LedgerError);
            log!(
                Priority::Info,
                "Failed to mint deposit {deposit_id}, retrying on the next round: {transient:?}"
            );
        }
        Err(ic_error) => {
            ScopeGuard::into_inner(quarantine_unless_defused);
            record_failed_mint_attempt(FailedMintReason::CallError);
            log!(
                Priority::Info,
                "Failed to mint deposit {deposit_id}, retrying on the next round: {ic_error}"
            );
        }
    }
}

fn is_beyond_deduplication_window(pending: &PendingMint, now: u64) -> bool {
    let age = Duration::from_nanos(now.saturating_sub(pending.created_at_time));
    age > LEDGER_DEDUPLICATION_WINDOW
}

fn mint_transfer_arg(deposit_id: DepositSolId, pending: &PendingMint) -> TransferArg {
    TransferArg {
        from_subaccount: None,
        to: pending.account(),
        fee: None,
        created_at_time: Some(pending.created_at_time),
        memo: Some(Memo::from(MintMemo::sweep(pending.sweep_signature(), deposit_id)).into()),
        amount: NumTokens::from(pending.amount_to_mint),
    }
}

fn record_minted_swept_deposit<R: CanisterRuntime>(
    runtime: &R,
    deposit_id: DepositSolId,
    mint_block_index: LedgerMintIndex,
) {
    mutate_state(|state| {
        process_event(
            state,
            EventType::MintedSweptDeposit {
                deposit_id,
                mint_block_index,
            },
            runtime,
        )
    });
}

fn record_quarantined_pending_mint<R: CanisterRuntime>(runtime: &R, deposit_id: DepositSolId) {
    mutate_state(|state| {
        process_event(
            state,
            EventType::QuarantinedPendingMint { deposit_id },
            runtime,
        )
    });
}
