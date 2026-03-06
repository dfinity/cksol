use crate::{
    guard::{TimerGuard, TimerGuardError},
    runtime::CanisterRuntime,
    state::read_state,
    state::{
        TaskType,
        audit::process_event,
        event::{DepositId, EventType},
        mutate_state,
    },
};
use cksol_types::{BurnMemo, DepositStatus, Lamport, Memo, MintMemo};
use derive_more::From;
use ic_canister_runtime::IcError;
use icrc_ledger_types::{
    icrc1::{
        account::Account,
        transfer::{NumTokens, TransferArg, TransferError},
    },
    icrc2::transfer_from::{TransferFromArgs, TransferFromError},
};
use num_traits::cast::ToPrimitive;
use scopeguard::ScopeGuard;
use solana_address::Address;
use std::time::Duration;
use thiserror::Error;

pub mod client;

// TODO DEFI-2643: Make this a configurable parameter
const MINT_RETRY_DELAY: Duration = Duration::from_mins(1);

pub fn schedule_mint_for_accepted_deposits<R: CanisterRuntime + Clone + 'static>(runtime: R) {
    runtime
        .clone()
        .set_timer(MINT_RETRY_DELAY, mint_for_accepted_deposits(runtime));
}

pub async fn mint_for_accepted_deposits<R: CanisterRuntime + Clone + 'static>(runtime: R) {
    // In case we do not manage to mint all pending deposits, make sure to re-schedule
    // this task to execute.
    let schedule_timer_guard = scopeguard::guard(runtime.clone(), |runtime| {
        schedule_mint_for_accepted_deposits(runtime);
    });

    let _guard = match TimerGuard::new(TaskType::Mint) {
        Ok(guard) => guard,
        Err(_) => return,
    };

    let mut pending_accepted_deposits = false;
    for (deposit_id, amount_to_mint) in read_state(|state| state.accepted_deposits()) {
        match mint(&runtime, deposit_id, amount_to_mint).await {
            Ok(DepositStatus::Processing(_)) | Err(_) => {
                pending_accepted_deposits = true;
            }
            Ok(DepositStatus::Minted { .. }) | Ok(DepositStatus::Quarantined(_)) => {}
        }
    }

    if !pending_accepted_deposits {
        // No more pending deposits to mint, defuse guard
        ScopeGuard::into_inner(schedule_timer_guard);
    }
}

pub async fn mint_for_deposit<R: CanisterRuntime>(
    runtime: &R,
    deposit_id: DepositId,
    amount_to_mint: Lamport,
) -> Result<DepositStatus, MintError> {
    let _guard = TimerGuard::new(TaskType::Mint)?;
    mint(runtime, deposit_id, amount_to_mint).await
}

/// This method should only be called after having acquired a [`TimerGuard`]
///  for [`TaskType::Mint`].
async fn mint<R: CanisterRuntime>(
    runtime: &R,
    deposit_id: DepositId,
    amount_to_mint: Lamport,
) -> Result<DepositStatus, MintError> {
    let signature = deposit_id.signature;
    let mint_memo = MintMemo::convert(signature);

    // Ensure that even if we were to panic in the callback, after having contacted the ledger
    // to mint the tokens, this deposit will not be processed again.
    let prevent_double_minting_guard = scopeguard::guard(deposit_id, |deposit_id| {
        mutate_state(|s| process_event(s, EventType::QuarantinedDeposit(deposit_id), runtime));
    });

    let client = read_state(|state| state.ledger_client(runtime.inter_canister_call_runtime()));
    let block_index = match client
        .transfer(TransferArg {
            from_subaccount: None,
            to: deposit_id.account,
            fee: None,
            created_at_time: Some(runtime.time()),
            memo: Some(Memo::from(mint_memo).into()),
            amount: NumTokens::from(amount_to_mint),
        })
        .await
        .map_err(MintError::from)
        .and_then(|r| r.map_err(MintError::from))
    {
        Ok(block_index) => block_index,
        Err(e) => {
            // Minting failed, defuse guard
            ScopeGuard::into_inner(prevent_double_minting_guard);
            return Err(e);
        }
    };

    mutate_state(|s| {
        process_event(
            s,
            EventType::Minted {
                deposit_id,
                mint_block_index: block_index,
            },
            runtime,
        )
    });

    // Minting succeeded, defuse guard
    ScopeGuard::into_inner(prevent_double_minting_guard);

    Ok(DepositStatus::Minted {
        block_index: *block_index.get(),
        minted_amount: amount_to_mint,
        signature: signature.into(),
    })
}

pub async fn burn<R: CanisterRuntime>(
    runtime: &R,
    minter_account: Account,
    from: Account,
    burn_amount: Lamport,
    to_address: Address,
) -> Result<u64, BurnError> {
    let burn_memo = BurnMemo::convert(to_address);

    let block_index =
        read_state(|state| state.ledger_client(runtime.inter_canister_call_runtime()))
            .transfer_from(TransferFromArgs {
                spender_subaccount: None,
                from,
                to: minter_account,
                fee: None,
                // TODO DEFI-2671 If we deduplicate we probably want to do it on the Account level with a guard
                // and not using the ledger deduplication mechanism.
                created_at_time: None,
                memo: Some(Memo::from(burn_memo).into()),
                amount: NumTokens::from(burn_amount),
            })
            .await??;

    Ok(block_index
        .0
        .to_u64()
        .expect("ledger block index does not fit into u64"))
}

#[derive(Debug, PartialEq, Error, From)]
#[from(IcError)]
pub enum MintError {
    #[error("Mint already in progress")]
    AlreadyProcessing(TimerGuardError),
    #[error("Error while calling ledger canister: {0}")]
    IcError(IcError),
    // TODO DEFI-2643: Should we panic on any of those errors?
    #[error("Failed to mint ckSOL: {0}")]
    TransferError(TransferError),
}

#[derive(Debug, PartialEq, Error, From)]
#[from(IcError)]
pub enum BurnError {
    #[error("Error while calling ledger canister: {0}")]
    IcError(IcError),
    #[error("Failed to burn ckSOL: {0}")]
    TransferFromError(TransferFromError),
}
