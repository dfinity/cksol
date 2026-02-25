use candid::Principal;
use ic_canister_runtime::{IcError, Runtime};
use icrc_ledger_types::{
    icrc1::transfer::{BlockIndex, TransferArg, TransferError},
    icrc2::transfer_from::{TransferFromArgs, TransferFromError},
};

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

impl<R: Runtime> LedgerClient<R> {
    pub async fn transfer_from(
        &self,
        args: TransferFromArgs,
    ) -> Result<Result<BlockIndex, TransferFromError>, IcError> {
        self.runtime
            .update_call(self.ledger_canister_id, "icrc2_transfer_from", (args,), 0)
            .await
    }
}
