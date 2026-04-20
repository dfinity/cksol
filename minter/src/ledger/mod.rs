use crate::{
    runtime::CanisterRuntime,
    state::read_state,
    state::{
        audit::process_event,
        event::{DepositId, EventType},
        mutate_state,
    },
};
use canlog::log;
use cksol_types::{BurnMemo, DepositStatus, Memo, MintMemo};
use cksol_types_internal::log::Priority;
use icrc_ledger_types::{
    icrc1::{
        account::Account,
        transfer::{NumTokens, TransferArg, TransferError},
    },
    icrc2::transfer_from::{TransferFromArgs, TransferFromError},
};
use num_traits::cast::ToPrimitive;
use scopeguard::ScopeGuard;
use sol_rpc_types::Lamport;
use solana_address::Address;
use thiserror::Error;

pub mod client;

pub async fn mint<R: CanisterRuntime>(
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
    {
        Ok(Ok(block_index)) => block_index,
        Ok(Err(transfer_error)) => {
            ScopeGuard::into_inner(prevent_double_minting_guard);
            return Err(parse_mint_transfer_error(transfer_error));
        }
        Err(ic_error) => {
            ScopeGuard::into_inner(prevent_double_minting_guard);
            return Err(MintError::TemporarilyUnavailable(format!(
                "Failed to mint tokens: {ic_error}"
            )));
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

    log!(
        Priority::Info,
        "Minted {amount_to_mint} lamports for deposit {deposit_id:?} (ledger block index {})",
        block_index.get()
    );

    // Minting succeeded, defuse guard
    ScopeGuard::into_inner(prevent_double_minting_guard);

    Ok(DepositStatus::Minted {
        block_index: *block_index.get(),
        minted_amount: amount_to_mint,
        deposit_id: deposit_id.into(),
    })
}

fn parse_mint_transfer_error(error: TransferError) -> MintError {
    match error {
        TransferError::TemporarilyUnavailable => {
            MintError::TemporarilyUnavailable("Ledger is temporarily unavailable".to_string())
        }
        TransferError::GenericError {
            error_code,
            message,
        } => MintError::TemporarilyUnavailable(format!(
            "Ledger returned a generic error: code {error_code}, message: {message}"
        )),
        TransferError::BadFee { expected_fee } => {
            panic!("BUG: unexpected BadFee error, expected_fee: {expected_fee}")
        }
        TransferError::BadBurn { min_burn_amount } => {
            panic!("BUG: unexpected BadBurn error, min_burn_amount: {min_burn_amount}")
        }
        TransferError::InsufficientFunds { balance } => {
            panic!("BUG: unexpected InsufficientFunds error, balance: {balance}")
        }
        TransferError::TooOld => panic!("BUG: unexpected TooOld error"),
        TransferError::CreatedInFuture { ledger_time } => {
            panic!("BUG: unexpected CreatedInFuture error, ledger_time: {ledger_time}")
        }
        TransferError::Duplicate { duplicate_of } => {
            panic!("BUG: unexpected Duplicate error, duplicate_of: {duplicate_of}")
        }
    }
}

pub async fn burn<R: CanisterRuntime>(
    runtime: &R,
    minter_account: Account,
    from: Account,
    burn_amount: Lamport,
    to_address: Address,
) -> Result<u64, BurnError> {
    let burn_memo = BurnMemo::convert(to_address);

    let result = read_state(|state| state.ledger_client(runtime.inter_canister_call_runtime()))
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
        .await;

    let block_index = match result {
        Ok(Ok(block_index)) => block_index,
        Ok(Err(transfer_from_error)) => {
            return Err(parse_burn_transfer_from_error(transfer_from_error));
        }
        Err(ic_error) => {
            return Err(BurnError::TemporarilyUnavailable(format!(
                "Failed to burn tokens: {ic_error}"
            )));
        }
    };

    let block_index = block_index
        .0
        .to_u64()
        .expect("ledger block index does not fit into u64");

    log!(
        Priority::Info,
        "Burned {burn_amount} lamports from {from:?} for withdrawal to {to_address} (ledger block index {block_index})"
    );

    Ok(block_index)
}

fn parse_burn_transfer_from_error(error: TransferFromError) -> BurnError {
    match error {
        TransferFromError::InsufficientFunds { balance } => BurnError::InsufficientFunds {
            balance: balance.0.to_u64().expect("balance should fit in u64"),
        },
        TransferFromError::InsufficientAllowance { allowance } => {
            BurnError::InsufficientAllowance {
                allowance: allowance.0.to_u64().expect("allowance should fit in u64"),
            }
        }
        TransferFromError::TemporarilyUnavailable => {
            BurnError::TemporarilyUnavailable("Ledger is temporarily unavailable".to_string())
        }
        TransferFromError::GenericError {
            error_code,
            message,
        } => BurnError::TemporarilyUnavailable(format!(
            "Ledger returned a generic error: code {error_code}, message: {message}"
        )),
        TransferFromError::BadFee { expected_fee } => {
            panic!("BUG: unexpected BadFee error, expected_fee: {expected_fee}")
        }
        TransferFromError::BadBurn { min_burn_amount } => {
            panic!("BUG: unexpected BadBurn error, min_burn_amount: {min_burn_amount}")
        }
        TransferFromError::TooOld => panic!("BUG: unexpected TooOld error"),
        TransferFromError::CreatedInFuture { ledger_time } => {
            panic!("BUG: unexpected CreatedInFuture error, ledger_time: {ledger_time}")
        }
        TransferFromError::Duplicate { duplicate_of } => {
            panic!("BUG: unexpected Duplicate error, duplicate_of: {duplicate_of}")
        }
    }
}

/// Errors that can occur when minting ckSOL tokens.
#[derive(Debug, PartialEq, Error)]
pub enum MintError {
    #[error("Failed to mint ckSOL: {0}")]
    TemporarilyUnavailable(String),
}

/// Errors that can occur when burning ckSOL tokens.
#[derive(Debug, PartialEq, Error)]
pub enum BurnError {
    #[error("Failed to burn ckSOL: {0}")]
    TemporarilyUnavailable(String),
    #[error("Insufficient funds to burn ckSOL, balance: {balance}")]
    InsufficientFunds { balance: Lamport },
    #[error("Insufficient allowance to burn ckSOL, allowance: {allowance}")]
    InsufficientAllowance { allowance: Lamport },
}
