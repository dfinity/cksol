use crate::{
    address::{associated_token_address, minter_address, minter_public_key},
    constants::CKSOL_LEDGER_TRANSACTION_FEE,
    deposit::mint::{MintQueue, mint_pending, process_pending_mints_round},
    guard::{GuardError, TimerGuard, withdrawal_guard},
    ledger::{self, BurnError, client::CkLedger, spl::SplBurnError},
    numeric::LedgerBurnIndex,
    runtime::CanisterRuntime,
    state::{
        PendingSplCkSolRefund, State, SupportedSplToken, TaskType,
        audit::process_event,
        event::{EventType, SplWithdrawalRequest, SplWithdrawalRequestTerms},
        mutate_state, read_state,
    },
};
use canlog::log;
use cksol_types::{Memo, MintMemo, SplRefundKind, WithdrawSplId, WithdrawSplOk, WithdrawSplStatus};
pub use cksol_types::{WithdrawSplError, WithdrawSplLedgerError as LedgerError};
use cksol_types_internal::{SolanaNetwork, log::Priority};
use icrc_ledger_types::{
    icrc1::{account::Account, transfer::TransferArg},
    icrc2::transfer_from::TransferFromError,
};
use sol_rpc_types::Lamport;
use solana_address::Address;
use std::{str::FromStr, time::Duration};

#[cfg(test)]
mod tests;

pub fn withdrawal_status(cksol_burn_block_index: WithdrawSplId) -> WithdrawSplStatus {
    read_state(|state| state.spl_withdrawal_status(cksol_burn_block_index))
}

fn cksol_ledger() -> CkLedger {
    read_state(|state| CkLedger {
        id: state.ledger_canister_id(),
        token_symbol: match state.solana_network() {
            SolanaNetwork::Mainnet => "ckSOL",
            SolanaNetwork::Devnet => "ckDevnetSOL",
            SolanaNetwork::Testnet => "ckTestnetSOL",
        }
        .to_string(),
    })
}

fn cksol_burn_error(error: BurnError, failed_burn_amount: u64, ledger: &CkLedger) -> LedgerError {
    match error {
        BurnError::TemporarilyUnavailable(message) => LedgerError::TemporarilyUnavailable(format!(
            "{} ledger: {message}",
            ledger.token_symbol
        )),
        BurnError::InsufficientFunds { balance } => LedgerError::InsufficientFunds {
            balance: balance.into(),
            failed_burn_amount: failed_burn_amount.into(),
            token_symbol: ledger.token_symbol.clone(),
            ledger_id: ledger.id,
        },
        BurnError::InsufficientAllowance { allowance } => LedgerError::InsufficientAllowance {
            allowance: allowance.into(),
            failed_burn_amount: failed_burn_amount.into(),
            token_symbol: ledger.token_symbol.clone(),
            ledger_id: ledger.id,
        },
    }
}

fn ckspl_burn_error(
    error: SplBurnError,
    failed_burn_amount: u64,
    ledger: &CkLedger,
) -> LedgerError {
    match error {
        SplBurnError::TransferFrom(TransferFromError::InsufficientFunds { balance }) => {
            LedgerError::InsufficientFunds {
                balance,
                failed_burn_amount: failed_burn_amount.into(),
                token_symbol: ledger.token_symbol.clone(),
                ledger_id: ledger.id,
            }
        }
        SplBurnError::TransferFrom(TransferFromError::InsufficientAllowance { allowance }) => {
            LedgerError::InsufficientAllowance {
                allowance,
                failed_burn_amount: failed_burn_amount.into(),
                token_symbol: ledger.token_symbol.clone(),
                ledger_id: ledger.id,
            }
        }
        SplBurnError::TransferFrom(TransferFromError::BadBurn { min_burn_amount }) => {
            LedgerError::AmountTooLow {
                minimum_burn_amount: min_burn_amount,
                failed_burn_amount: failed_burn_amount.into(),
                token_symbol: ledger.token_symbol.clone(),
                ledger_id: ledger.id,
            }
        }
        error => {
            LedgerError::TemporarilyUnavailable(format!("{} ledger: {error}", ledger.token_symbol))
        }
    }
}

impl From<super::DestinationError> for WithdrawSplError {
    fn from(error: super::DestinationError) -> Self {
        match error {
            super::DestinationError::InvalidDestination(message) => {
                Self::InvalidDestination(message)
            }
            super::DestinationError::TemporarilyUnavailable(message) => {
                Self::TemporarilyUnavailable(message)
            }
        }
    }
}

impl From<GuardError> for WithdrawSplError {
    fn from(error: GuardError) -> Self {
        match error {
            GuardError::AlreadyProcessing => Self::AlreadyProcessing,
            GuardError::TooManyConcurrentRequests => {
                Self::TemporarilyUnavailable("too many concurrent requests".to_string())
            }
        }
    }
}

/// Selects the token ledger and checks the amount in the token's smallest units.
/// The minter enforces no minimum: the amount only has to exceed the token fee, so that
/// some tokens are transferred. The token ledger rejects a burn below its own minimum.
pub fn validate_token(mint: Address, amount: u64) -> Result<SupportedSplToken, WithdrawSplError> {
    let token = read_state(|state| state.supported_spl_token(&mint).cloned())
        .ok_or(WithdrawSplError::TokenNotSupported { mint: mint.into() })?;
    if amount <= token.withdrawal_fee_token {
        return Err(WithdrawSplError::ValueTooSmall {
            minimum_withdrawal_amount: token.withdrawal_fee_token.saturating_add(1),
            withdrawal_amount: amount,
        });
    }
    Ok(token)
}

/// Validates a destination wallet locally, before either ledger burn.
/// The state of its token account is not read: the transaction creates it if needed.
pub fn validate_destination(
    token: &SupportedSplToken,
    address: &str,
) -> Result<Address, WithdrawSplError> {
    let destination = Address::from_str(address)
        .map_err(|error| WithdrawSplError::MalformedAddress(error.to_string()))?;
    super::validate_destination(&destination)?;

    let master_key = minter_public_key()
        .map_err(|error| WithdrawSplError::TemporarilyUnavailable(error.to_string()))?;
    let treasury = associated_token_address(&minter_address(&master_key), &token.mint);
    if destination == treasury {
        return Err(WithdrawSplError::InvalidDestination(format!(
            "{destination} is the ckSOL minter's treasury token account for {}",
            token.mint,
        )));
    }
    // An associated token account can be derived only for an owner that is on the curve.
    if !destination.is_on_curve() {
        return Err(WithdrawSplError::InvalidDestination(format!(
            "{destination} is not a wallet address: it is off the ed25519 curve"
        )));
    }
    Ok(destination)
}

/// Validates a withdrawal, burns its fee and tokens, then records acceptance.
/// The account guard is held across both ledger calls.
pub async fn withdraw<R: CanisterRuntime>(
    runtime: &R,
    account: Account,
    mint: Address,
    amount: u64,
    address: &str,
) -> Result<WithdrawSplOk, WithdrawSplError> {
    let _guard = withdrawal_guard(account)?;
    let token = validate_token(mint, amount)?;
    let address = validate_destination(&token, address)?;
    super::validate_nonce_pool_not_empty()?;
    let (fee, rent_reserve) = read_state(|state| {
        (
            state.spl_withdrawal_fee(),
            state.spl_withdrawal_rent_reserve(),
        )
    });
    let token_ledger = CkLedger {
        id: token.ledger_id,
        token_symbol: token.symbol.clone(),
    };
    let fee_ledger = cksol_ledger();
    let request = SplWithdrawalRequestTerms {
        account,
        mint: token.mint,
        ledger_id: token.ledger_id,
        address,
        amount,
        fee,
        rent_reserve,
        token_fee: token.withdrawal_fee_token,
    };
    let cksol_burn_amount = request.cksol_burn_amount().ok_or_else(|| {
        WithdrawSplError::TemporarilyUnavailable(
            "SPL withdrawal fee exceeds the maximum lamport amount".to_string(),
        )
    })?;

    let cksol_burn_block_index = burn_withdrawal_fee(runtime, &request, cksol_burn_amount)
        .await
        .map_err(|error| WithdrawSplError::CkSolLedgerError {
            error: cksol_burn_error(error, cksol_burn_amount, &fee_ledger),
        })?;
    match burn_withdrawal_amount(runtime, cksol_burn_block_index, &request).await {
        Ok(receipt) => Ok(receipt),
        Err(error) => {
            schedule_withdrawal_fee_reimbursement(
                runtime,
                cksol_burn_block_index,
                &request,
                cksol_burn_amount,
                &error,
                CKSOL_LEDGER_TRANSACTION_FEE,
            );
            Err(WithdrawSplError::CkSplLedgerError {
                cksol_burn_block_index: *cksol_burn_block_index.get(),
                error: ckspl_burn_error(error, request.amount, &token_ledger),
            })
        }
    }
}

/// Burns the ckSOL fee and rent reserve before attempting the token burn.
/// The caller must validate the terms and hold the account withdrawal guard
/// across both burns.
pub async fn burn_withdrawal_fee<R: CanisterRuntime>(
    runtime: &R,
    request: &SplWithdrawalRequestTerms,
    cksol_burn_amount: Lamport,
) -> Result<LedgerBurnIndex, BurnError> {
    let burn_index = ledger::burn(
        runtime,
        runtime.canister_self().into(),
        request.account,
        cksol_burn_amount,
        Memo::spl_withdrawal_fee(request.mint, request.amount, request.address),
    )
    .await?;
    Ok(LedgerBurnIndex::new(burn_index))
}

/// Burns the token amount, then records acceptance with both burn indices.
/// The caller holds the account withdrawal guard across both burns.
pub async fn burn_withdrawal_amount<R: CanisterRuntime>(
    runtime: &R,
    cksol_burn_block_index: LedgerBurnIndex,
    request: &SplWithdrawalRequestTerms,
) -> Result<WithdrawSplOk, SplBurnError> {
    let ckspl_burn_block_index = ledger::spl::burn(
        runtime,
        request.ledger_id,
        request.account,
        request.amount,
        Memo::spl_convert(*cksol_burn_block_index.get(), request.address),
    )
    .await?;
    let accepted = SplWithdrawalRequest {
        terms: request.clone(),
        cksol_burn_block_index,
        ckspl_burn_block_index: LedgerBurnIndex::new(ckspl_burn_block_index),
    };
    mutate_state(|state| {
        process_event(
            state,
            EventType::AcceptedSplWithdrawalRequest(accepted.clone()),
            runtime,
        );
    });
    log!(
        Priority::Info,
        "Accepted SPL withdrawal request from {:?}: burned {} units of mint {}, queued withdrawal to {} (ckSOL burn block index {cksol_burn_block_index}, ckSPL burn block index {ckspl_burn_block_index})",
        accepted.terms.account,
        accepted.terms.amount,
        accepted.terms.mint,
        accepted.terms.address,
    );
    Ok(WithdrawSplOk {
        cksol_burn_block_index: *cksol_burn_block_index.get(),
        ckspl_burn_block_index,
    })
}

/// Queues a ckSOL refund after a returned token-burn error, as in ckERC20.
/// The caller holds the withdrawal guard. Call errors and temporary or generic
/// ledger errors receive a full refund; user errors deduct the ledger fee.
pub fn schedule_withdrawal_fee_reimbursement<R: CanisterRuntime>(
    runtime: &R,
    cksol_burn_block_index: LedgerBurnIndex,
    request: &SplWithdrawalRequestTerms,
    cksol_burn_amount: Lamport,
    error: &SplBurnError,
    cksol_ledger_fee: Lamport,
) {
    let reimbursed_amount = match error {
        SplBurnError::Call(_)
        | SplBurnError::TransferFrom(
            TransferFromError::TemporarilyUnavailable | TransferFromError::GenericError { .. },
        ) => cksol_burn_amount,
        SplBurnError::TransferFrom(
            TransferFromError::InsufficientFunds { .. }
            | TransferFromError::InsufficientAllowance { .. }
            | TransferFromError::BadBurn { .. },
        ) => cksol_burn_amount.saturating_sub(cksol_ledger_fee),
        SplBurnError::TransferFrom(
            TransferFromError::BadFee { .. }
            | TransferFromError::TooOld
            | TransferFromError::CreatedInFuture { .. }
            | TransferFromError::Duplicate { .. },
        ) => return,
    };
    if reimbursed_amount == 0 {
        return;
    }
    // The refund is recorded with its outcome, before any ledger call.
    let created_at_time = runtime.time();
    mutate_state(|state| {
        process_event(
            state,
            EventType::FailedSplWithdrawalRequest {
                cksol_burn_block_index,
                to: request.account,
                reimbursed_amount,
            },
            runtime,
        );
        process_event(
            state,
            EventType::QueuedSplCkSolRefund {
                cksol_burn_block_index,
                kind: SplRefundKind::FailedTokenBurn,
                to: request.account,
                amount: reimbursed_amount,
                created_at_time,
            },
            runtime,
        );
    });
    log!(
        Priority::Info,
        "Scheduled reimbursement of {reimbursed_amount} lamports of ckSOL to {:?} for SPL withdrawal {cksol_burn_block_index} after its token burn returned an error",
        request.account,
    );
}

/// Builds the fixed mint arguments of a ckSOL refund, so a retry is deduplicated.
pub fn cksol_refund_transfer_arg(
    cksol_burn_block_index: WithdrawSplId,
    pending: &PendingSplCkSolRefund,
) -> TransferArg {
    TransferArg {
        from_subaccount: None,
        to: pending.to,
        fee: None,
        created_at_time: Some(pending.created_at_time),
        memo: Some(
            Memo::from(MintMemo::spl_cksol_refund(
                cksol_burn_block_index,
                pending.kind,
            ))
            .into(),
        ),
        amount: pending.amount.into(),
    }
}

/// Mints the queued ckSOL refunds in bounded rounds, like the deposit mints.
pub async fn process_pending_cksol_refunds<R: CanisterRuntime>(runtime: R) {
    let _guard = match TimerGuard::new(TaskType::RefundSplCkSol) {
        Ok(guard) => guard,
        Err(_) => return,
    };

    let run_again = process_pending_mints_round(
        &runtime,
        MintQueue::SplCkSolRefund,
        |state: &State| state.pending_spl_cksol_refunds(),
        process_pending_cksol_refund,
    )
    .await;

    if run_again {
        runtime.set_timer(Duration::ZERO, process_pending_cksol_refunds);
    }
}

async fn process_pending_cksol_refund<R: CanisterRuntime>(
    runtime: &R,
    cksol_burn_block_index: WithdrawSplId,
    pending: PendingSplCkSolRefund,
) {
    let client = read_state(|state| state.ledger_client(runtime.inter_canister_call_runtime()));
    let burn_index = LedgerBurnIndex::new(cksol_burn_block_index);
    let kind = pending.kind;
    mint_pending(
        runtime,
        MintQueue::SplCkSolRefund,
        client,
        cksol_refund_transfer_arg(cksol_burn_block_index, &pending),
        |mint_block_index| EventType::ReimbursedSplCkSol {
            cksol_burn_block_index: burn_index,
            kind,
            mint_block_index,
        },
        EventType::QuarantinedSplCkSolRefund {
            cksol_burn_block_index: burn_index,
            kind,
        },
        &format!("ckSOL refund of SPL withdrawal {cksol_burn_block_index}"),
    )
    .await;
}
