use crate::{
    deposit::mint::{MintQueue, mint_pending, process_pending_mints_round},
    guard::TimerGuard,
    runtime::CanisterRuntime,
    state::{PendingMint, TaskType, event::EventType, read_state},
};
use cksol_types::{DepositSolId, Memo, MintMemo};
use icrc_ledger_types::icrc1::transfer::{NumTokens, TransferArg};
use std::time::Duration;

#[cfg(test)]
mod tests;

pub async fn process_pending_mints<R: CanisterRuntime>(runtime: R) {
    let _guard = match TimerGuard::new(TaskType::Mint) {
        Ok(guard) => guard,
        Err(_) => return,
    };

    let run_again = process_pending_mints_round(
        &runtime,
        MintQueue::Sol,
        |state| state.deposits().pending_mints(),
        process_pending_mint,
    )
    .await;

    if run_again {
        runtime.set_timer(Duration::ZERO, process_pending_mints);
    }
}

async fn process_pending_mint<R: CanisterRuntime>(
    runtime: &R,
    deposit_id: DepositSolId,
    pending: PendingMint,
) {
    let client = read_state(|state| state.ledger_client(runtime.inter_canister_call_runtime()));
    mint_pending(
        runtime,
        MintQueue::Sol,
        client,
        mint_transfer_arg(deposit_id, &pending),
        |mint_block_index| EventType::MintedSweptDeposit {
            deposit_id,
            mint_block_index,
        },
        EventType::QuarantinedPendingMint { deposit_id },
        &format!("deposit {deposit_id}"),
    )
    .await;
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
