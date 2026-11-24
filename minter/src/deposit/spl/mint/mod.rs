use crate::{
    deposit::mint::{MintQueue, mint_pending, process_pending_mints_round},
    guard::TimerGuard,
    ledger::client::LedgerClient,
    runtime::CanisterRuntime,
    state::{PendingSplMint, TaskType, event::EventType, read_state},
};
use cksol_types::{DepositSplId, Memo, MintMemo};
use icrc_ledger_types::icrc1::transfer::{NumTokens, TransferArg};
use std::time::Duration;

#[cfg(test)]
mod tests;

/// Mints the net amounts of credited SPL deposits, each on the ledger of its token.
/// The guard is separate from the ckSOL mints: a ledger that does not reply blocks only this task.
pub async fn process_pending_spl_mints<R: CanisterRuntime>(runtime: R) {
    let _guard = match TimerGuard::new(TaskType::MintSpl) {
        Ok(guard) => guard,
        Err(_) => return,
    };

    let run_again = process_pending_mints_round(
        &runtime,
        MintQueue::Spl,
        |state| state.spl_deposits().pending_mints(),
        process_pending_spl_mint,
    )
    .await;

    if run_again {
        runtime.set_timer(Duration::ZERO, process_pending_spl_mints);
    }
}

async fn process_pending_spl_mint<R: CanisterRuntime>(
    runtime: &R,
    deposit_id: DepositSplId,
    pending: PendingSplMint,
) {
    let ledger_id = read_state(|state| {
        state
            .supported_spl_token(&pending.deposit.deposit.mint)
            .expect("BUG: a pending SPL mint must have a registered token")
            .ledger_id
    });
    mint_pending(
        runtime,
        MintQueue::Spl,
        LedgerClient::new(runtime.inter_canister_call_runtime(), ledger_id),
        mint_transfer_arg(deposit_id, &pending),
        |mint_block_index| EventType::MintedSweptSplDeposit {
            deposit_id,
            mint_block_index,
        },
        EventType::QuarantinedPendingSplMint { deposit_id },
        &format!("SPL deposit {deposit_id}"),
    )
    .await;
}

fn mint_transfer_arg(deposit_id: DepositSplId, pending: &PendingSplMint) -> TransferArg {
    TransferArg {
        from_subaccount: None,
        to: pending.account(),
        fee: None,
        // Each retry sends the credit timestamp, so the ledger deduplicates it.
        created_at_time: Some(pending.created_at_time),
        memo: Some(Memo::from(MintMemo::sweep(pending.sweep_signature(), deposit_id)).into()),
        amount: NumTokens::from(pending.amount_to_mint),
    }
}
