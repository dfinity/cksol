use crate::{
    runtime::CanisterRuntime,
    state::read_state,
    state::{
        audit::process_event,
        event::{Deposit, EventType, MintedEvent},
        mutate_state,
    },
};
use cksol_types::{DepositStatus, Memo, MintMemo};
use derive_more::From;
use ic_canister_runtime::IcError;
use icrc_ledger_types::icrc1::transfer::{NumTokens, TransferArg, TransferError};
use scopeguard::ScopeGuard;
use thiserror::Error;

pub mod client;

pub async fn mint<R: CanisterRuntime>(
    runtime: &R,
    deposit: Deposit,
) -> Result<DepositStatus, MintError> {
    let signature = deposit.deposit_id.signature;
    let mint_memo = MintMemo::convert(signature);
    let minted_amount = deposit.amount_to_mint;

    // Ensure that even if we were to panic in the callback, after having contacted the ledger
    // to mint the tokens, this deposit will not be processed again.
    let prevent_double_minting_guard = scopeguard::guard(deposit.clone(), |deposit| {
        mutate_state(|s| process_event(s, EventType::QuarantinedDeposit(deposit), runtime));
    });

    let client = read_state(|state| state.ledger_client(runtime.inter_canister_call_runtime()));
    let block_index = match client
        .transfer(TransferArg {
            from_subaccount: None,
            to: deposit.deposit_id.account,
            fee: None,
            created_at_time: Some(runtime.time()),
            memo: Some(Memo::from(mint_memo).into()),
            amount: NumTokens::from(minted_amount),
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
            EventType::Minted(MintedEvent {
                deposit,
                minted_amount,
                mint_block_index: block_index,
            }),
            runtime,
        )
    });

    // Minting succeeded, defuse guard
    ScopeGuard::into_inner(prevent_double_minting_guard);

    Ok(DepositStatus::Minted {
        block_index: *block_index.get(),
        minted_amount,
        signature: signature.into(),
    })
}

#[derive(Debug, PartialEq, Error, From)]
#[from(IcError)]
pub enum MintError {
    #[error("Error while calling ledger canister: {0}")]
    IcError(IcError),
    // TODO DEFI-2643: Should we panic on any of those errors?
    #[error("Failed to mint ckSOL: {0}")]
    TransferError(TransferError),
}
