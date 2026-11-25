//! Classification of failed SPL sweeps for the `failed_spl_sweeps` metric.
use crate::{rpc::SubmitTransactionError, storage::FailedSplSweepReason};
use sol_rpc_types::RpcError;
use solana_instruction::error::InstructionError;
use solana_transaction::TransactionError;
use spl_token_interface::error::TokenError;

/// The JSON-RPC error code of a transaction that the preflight simulation rejected.
const PREFLIGHT_FAILURE_CODE: i64 = -32002;
const PREFLIGHT_FAILURE_PREFIX: &str = "Transaction simulation failed: ";

/// The sweep has one instruction, the token transfer.
const FROZEN_ACCOUNT: TransactionError = TransactionError::InstructionError(
    0,
    InstructionError::Custom(TokenError::AccountFrozen as u32),
);
/// The token program cannot decode an account that does not exist.
const MISSING_ACCOUNT: TransactionError =
    TransactionError::InstructionError(0, InstructionError::InvalidAccountData);

/// The reason of a sweep that failed on-chain.
pub fn failed_sweep_reason(error: &TransactionError) -> FailedSplSweepReason {
    if *error == FROZEN_ACCOUNT {
        FailedSplSweepReason::FrozenAccount
    } else if *error == MISSING_ACCOUNT {
        FailedSplSweepReason::MissingAccount
    } else {
        FailedSplSweepReason::Other
    }
}

/// The reason of a sweep that the preflight simulation rejected, so it was not broadcast.
/// Returns `None` for other submission errors, because their transaction may still land.
pub fn preflight_rejection_reason(error: &SubmitTransactionError) -> Option<FailedSplSweepReason> {
    let SubmitTransactionError::RpcError(RpcError::JsonRpcError(error)) = error else {
        return None;
    };
    if error.code != PREFLIGHT_FAILURE_CODE {
        return None;
    }
    // The provider reports the simulation error only as its display text.
    let reason = [FROZEN_ACCOUNT, MISSING_ACCOUNT]
        .iter()
        .find(|cause| error.message == format!("{PREFLIGHT_FAILURE_PREFIX}{cause}"))
        .map(failed_sweep_reason)
        .unwrap_or(FailedSplSweepReason::Other);
    Some(reason)
}
