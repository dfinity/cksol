use cksol_types::{Address, RetrieveSolError, RetrieveSolOk};
use icrc_ledger_types::icrc1::account::Account;

use crate::{ledger::burn, runtime::CanisterRuntime};

pub async fn retrieve_sol<R: CanisterRuntime>(
    runtime: R,
    from: Account,
    amount: u64,
    to: Address,
) -> Result<RetrieveSolOk, RetrieveSolError> {
    let block_index = burn(&runtime, from, amount, to)
        .await
        .map_err(|_e| RetrieveSolError::InsufficientFunds { balance: 0 })?;
    Ok(RetrieveSolOk { block_index })
}
