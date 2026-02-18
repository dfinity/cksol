use crate::state::read_state;
use candid::Principal;
use canlog::log;
use cksol_types::{DepositStatus, Memo, MintMemo, UpdateBalanceError};
use cksol_types_internal::log::Priority;
use ic_canister_runtime::{IcError, Runtime};
use icrc_ledger_types::icrc1::{
    account::Account,
    transfer::{BlockIndex, NumTokens, TransferArg, TransferError},
};
use num_traits::cast::ToPrimitive;
use sol_rpc_types::{Lamport, Signature};

pub async fn mint<R: Runtime>(
    runtime: R,
    account: Account,
    deposit_amount: Lamport,
    deposit_transaction: Signature,
) -> Result<DepositStatus, UpdateBalanceError> {
    let mint_memo = MintMemo::convert(deposit_transaction.clone());
    let mint_result = LedgerClient::new(runtime, read_state(|state| state.ledger_canister_id()))
        .transfer(TransferArg {
            from_subaccount: None,
            to: account,
            fee: None,
            created_at_time: None,
            memo: Some(Memo::from(mint_memo).into()),
            amount: NumTokens::from(deposit_amount),
        })
        .await
        .map_err(|e| {
            UpdateBalanceError::TemporarilyUnavailable(format!(
                "Failed to send a message to the ledger: {e:?}"
            ))
        })?;
    let block_index = match mint_result {
        Ok(block_index) => block_index,
        Err(e) => {
            log!(
                Priority::Info,
                "Failed to mint ckSOL for transaction {deposit_transaction}: {e:?}",
            );
            return Ok(DepositStatus::Processing(deposit_transaction));
        }
    };
    Ok(DepositStatus::Minted {
        block_index: block_index
            .0
            .to_u64()
            .expect("ledger block index does not fit into u64"),
        minted_amount: deposit_amount,
        signature: deposit_transaction,
    })
}

pub struct LedgerClient<R> {
    pub runtime: R,
    pub ledger_canister_id: Principal,
}

impl<R> LedgerClient<R> {
    pub fn new(runtime: R, ledger_canister_id: Principal) -> Self {
        Self {
            runtime,
            ledger_canister_id,
        }
    }
}

impl<R: Runtime> LedgerClient<R> {
    pub async fn transfer(
        &self,
        args: TransferArg,
    ) -> Result<Result<BlockIndex, TransferError>, IcError> {
        self.runtime
            .update_call(self.ledger_canister_id, "icrc1_transfer", (args,), 0)
            .await
    }
}
