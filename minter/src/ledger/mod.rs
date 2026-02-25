use crate::{runtime::CanisterRuntime, state::read_state};
use cksol_types::{Address, BurnMemo, DepositStatus, Memo, MintMemo};
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
use sol_rpc_types::{Lamport, Signature};
use thiserror::Error;

pub mod client;

pub async fn mint<R: CanisterRuntime>(
    runtime: &R,
    account: Account,
    deposit_amount: Lamport,
    deposit_transaction: Signature,
) -> Result<DepositStatus, MintError> {
    let mint_memo = MintMemo::convert(deposit_transaction.clone());
    let block_index =
        read_state(|state| state.ledger_client(runtime.inter_canister_call_runtime()))
            .transfer(TransferArg {
                from_subaccount: None,
                to: account,
                fee: None,
                created_at_time: Some(runtime.time()),
                memo: Some(Memo::from(mint_memo).into()),
                amount: NumTokens::from(deposit_amount),
            })
            .await??;
    // TODO DEFI-2643: Record mint event
    Ok(DepositStatus::Minted {
        block_index: block_index
            .0
            .to_u64()
            .expect("ledger block index does not fit into u64"),
        minted_amount: deposit_amount,
        signature: deposit_transaction,
    })
}

pub async fn burn<R: CanisterRuntime>(
    runtime: &R,
    from: Account,
    burn_amount: Lamport,
    to_address: Address,
) -> Result<u64, BurnError> {
    let burn_memo = BurnMemo::convert(to_address);
    let minter_account: Account = ic_cdk::api::canister_self().into();

    let block_index =
        read_state(|state| state.ledger_client(runtime.inter_canister_call_runtime()))
            .transfer_from(TransferFromArgs {
                spender_subaccount: None,
                from,
                to: minter_account,
                fee: None,
                created_at_time: Some(runtime.time()),
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
