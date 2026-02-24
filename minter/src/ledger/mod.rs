use crate::{runtime::CanisterRuntime, state::read_state};
use cksol_types::{DepositStatus, Memo, MintMemo};
use derive_more::From;
use ic_canister_runtime::IcError;
use icrc_ledger_types::icrc1::{
    account::Account,
    transfer::{NumTokens, TransferArg, TransferError},
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

#[derive(Debug, PartialEq, Error, From)]
#[from(IcError)]
pub enum MintError {
    #[error("Error while calling ledger canister: {0}")]
    IcError(IcError),
    // TODO DEFI-2643: Should we panic on any of those errors?
    #[error("Failed to mint ckSOL: {0}")]
    TransferError(TransferError),
}
