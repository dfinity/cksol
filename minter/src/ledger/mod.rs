use crate::{
    runtime::CanisterRuntime,
    state::read_state,
    state::{
        audit::process_event,
        event::{AcceptedDepositEvent, EventType, MintedEvent},
        mutate_state,
    },
};
use cksol_types::{DepositStatus, Memo, MintMemo};
use derive_more::From;
use ic_canister_runtime::IcError;
use icrc_ledger_types::icrc1::transfer::{NumTokens, TransferArg, TransferError};
use thiserror::Error;

pub mod client;

pub async fn mint<R: CanisterRuntime>(
    runtime: &R,
    deposit_event: AcceptedDepositEvent,
) -> Result<DepositStatus, MintError> {
    let signature = deposit_event.signature;
    let mint_memo = MintMemo::convert(signature);

    let deposit_fee = read_state(|state| state.deposit_fee());
    assert!(
        deposit_event.amount > deposit_fee,
        "Deposit amount is less than fee!"
    );
    let minted_amount = deposit_event.amount - deposit_fee;

    let block_index =
        read_state(|state| state.ledger_client(runtime.inter_canister_call_runtime()))
            .transfer(TransferArg {
                from_subaccount: None,
                to: deposit_event.account,
                fee: None,
                created_at_time: Some(runtime.time()),
                memo: Some(Memo::from(mint_memo).into()),
                amount: NumTokens::from(minted_amount),
            })
            .await??;

    mutate_state(|s| {
        process_event(
            s,
            EventType::Minted(MintedEvent {
                deposit_event,
                minted_amount,
                mint_block_index: block_index,
            }),
            runtime,
        )
    });

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
