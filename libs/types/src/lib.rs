//! Candid types used by the Candid interface of the ckSOL minter.

#![forbid(unsafe_code)]
#![forbid(missing_docs)]

use candid::{CandidType, Nat, Principal};
use icrc_ledger_types::icrc1::account::{Account, Subaccount};
pub use memo::{BurnMemo, MAX_SERIALIZED_MEMO_BYTES, Memo, MintMemo};
use serde::{Deserialize, Serialize};
pub use sol_rpc_types::{Lamport, Pubkey as Address, Signature};
use thiserror::Error;

mod memo;

/// A single transaction can deposit to multiple accounts, so the signature alone is not sufficient.
/// The combination of a Solana transaction signature and the account it targets together
/// uniquely identify a deposit. If a transaction contains multiple transfers to the same account,
/// they are aggregated into a single deposit.
#[derive(Clone, Eq, PartialEq, Debug, CandidType, Deserialize, Serialize)]
pub struct DepositId {
    /// The Solana transaction signature.
    pub signature: Signature,
    /// The account to which the deposit is attributed.
    pub account: Account,
}

/// The outcome of processing a Solana deposit transaction.
#[derive(Clone, Eq, PartialEq, Debug, CandidType, Deserialize, Serialize)]
pub enum DepositStatus {
    /// The transaction is a valid deposit, but the corresponding ckSOL tokens
    /// have not yet been minted.
    Processing {
        /// The deposit amount.
        deposit_amount: Lamport,
        /// The amount to mint (deposit amount minus fees).
        amount_to_mint: Lamport,
        /// The deposit identifier.
        deposit_id: DepositId,
    },
    /// The transaction is a valid deposit, but it is unknown whether the
    /// corresponding ckSOL tokens have been minted, most likely because there
    /// was an unexpected panic while trying to mint.
    ///
    /// The deposit is quarantined to avoid any double minting and will not
    /// be further processed without manual intervention.
    Quarantined(DepositId),
    /// The minter accepted the deposit and minted ckSOL tokens on the ledger.
    Minted {
        /// The mint transaction index on the ledger.
        block_index: u64,
        /// The minted amount (deposit amount minus fees).
        minted_amount: Lamport,
        /// The deposit identifier.
        deposit_id: DepositId,
    },
}

/// Arguments for a request to the `get_deposit_address` ckSOL minter endpoint.
#[derive(Clone, Eq, PartialEq, Debug, Default, CandidType, Deserialize, Serialize)]
pub struct GetDepositAddressArgs {
    /// The principal to deposit funds to.
    ///
    /// If not set, defaults to the caller's principal.
    /// The resolved owner must be a non-anonymous principal.
    pub owner: Option<Principal>,
    /// The subaccount to deposit funds to.
    pub subaccount: Option<Subaccount>,
}

impl From<Account> for GetDepositAddressArgs {
    fn from(account: Account) -> Self {
        Self {
            owner: Some(account.owner),
            subaccount: account.subaccount,
        }
    }
}

/// Arguments for a request to the `process_deposit` ckSOL minter endpoint.
#[derive(Clone, Eq, PartialEq, Debug, CandidType, Deserialize, Serialize)]
pub struct ProcessDepositArgs {
    /// The principal to credit with the deposit.
    ///
    /// If not set, defaults to the caller's principal.
    /// The resolved owner must be a non-anonymous principal.
    pub owner: Option<Principal>,
    /// The subaccount to credit with the deposit.
    pub subaccount: Option<Subaccount>,
    /// Signature of the deposit transaction.
    pub signature: Signature,
}

/// An error from the `process_deposit` ckSOL minter endpoint.
#[derive(Debug, Clone, PartialEq, CandidType, Deserialize, Error)]
pub enum ProcessDepositError {
    /// Insufficient cycles attached by the caller to complete the [`process_deposit`] call.
    #[error(transparent)]
    InsufficientCycles(#[from] InsufficientCyclesError),
    /// The minter experiences temporary issues, try the call again later.
    #[error("Transient error, try the call again later: {0}")]
    TemporarilyUnavailable(String),
    /// There is already a concurrent `process_deposit` invocation from the same caller.
    #[error("There is already a concurrent `process_deposit` invocation from the same caller")]
    AlreadyProcessing,
    /// No matching transaction was found for the given signature.
    ///
    /// This can also happen if the transaction is not yet finalized, in which case trying
    /// this call again later may result in a successful mint.
    #[error("No transaction found for the given signature")]
    TransactionNotFound,
    /// The Solana transaction with the given signature is not a valid
    /// deposit to the owner's deposit address.
    #[error("The transaction is not a valid deposit: {0}")]
    InvalidDepositTransaction(String),
    /// The deposit amount does not cover the deposit fee.
    #[error(
        "Insufficient deposit amount: expected at least {minimum_deposit_amount} lamports, but got {deposit_amount} lamports"
    )]
    ValueTooSmall {
        /// The minimum deposit amount for the deposit to be accepted.
        minimum_deposit_amount: Lamport,
        /// The amount that was deposited.
        deposit_amount: Lamport,
    },
}

/// Arguments for a request to the `update_balance` ckSOL minter endpoint.
#[derive(Clone, Eq, PartialEq, Debug, CandidType, Deserialize, Serialize)]
pub struct UpdateBalanceArgs {
    /// The principal to register for automated deposit monitoring.
    ///
    /// If not set, defaults to the caller's principal.
    /// The resolved owner must be a non-anonymous principal.
    pub owner: Option<Principal>,
    /// The subaccount to register for automated deposit monitoring.
    pub subaccount: Option<Subaccount>,
}

/// An error from the `update_balance` ckSOL minter endpoint.
#[derive(Debug, Clone, PartialEq, CandidType, Deserialize, Error)]
pub enum UpdateBalanceError {
    /// The monitored account queue is at capacity.
    #[error("The monitored account queue is at capacity")]
    QueueFull,
}

/// Insufficient cycles attached by the caller to complete the call.
#[derive(Debug, Clone, PartialEq, CandidType, Deserialize, Error)]
#[error("Insufficient cycles attached, expected {expected} but got {received}")]
pub struct InsufficientCyclesError {
    /// The amount of cycles the call requires.
    pub expected: u128,
    /// The amount of cycles received by the minter (attached by the caller).
    pub received: u128,
}

/// Arguments for a withdrawal request to the ckSOL minter endpoint.
#[derive(Clone, Eq, PartialEq, Debug, Default, CandidType, Deserialize, Serialize)]
pub struct WithdrawalArgs {
    /// The subaccount to burn ckSOL from.
    pub from_subaccount: Option<Subaccount>,

    /// Amount to withdraw in Lamports.
    pub amount: u64,

    /// Address where to send Solana tokens.
    pub address: String,
}

/// The successful result of a withdrawal request.
#[derive(Clone, Eq, PartialEq, Debug, CandidType, Deserialize)]
pub struct WithdrawalOk {
    /// The index of the burn block on the ckSOL ledger.
    pub block_index: u64,
}

/// The error result of a withdrawal request.
#[derive(Clone, Eq, PartialEq, Debug, CandidType, Deserialize)]
pub enum WithdrawalError {
    /// There is another request for this principal.
    AlreadyProcessing,
    /// The withdrawal amount is too low.
    ValueTooSmall {
        /// The minimum withdrawal amount.
        minimum_withdrawal_amount: Lamport,
        /// The requested withdrawal amount.
        withdrawal_amount: Lamport,
    },
    /// The Solana address is not valid.
    MalformedAddress(String),
    /// The withdrawal account does not hold the requested ckSOL amount.
    InsufficientFunds {
        /// The current balance of the withdrawal account.
        balance: u64,
    },
    /// The minter is not approved to transfer the requested amount.
    InsufficientAllowance {
        /// The current allowance for the minter.
        allowance: u64,
    },
    /// There are too many concurrent requests, retry later.
    TemporarilyUnavailable(String),
}

/// Idetifier for a Solana transaction.
#[derive(Clone, Eq, PartialEq, Hash, Debug, CandidType, Deserialize)]
pub struct SolTransaction {
    /// The transaction hash of the Solana transaction.
    pub transaction_hash: String,
}

/// Status of a finalized transaction.
#[derive(Clone, Eq, PartialEq, Hash, Debug, CandidType, Deserialize)]
pub enum TxFinalizedStatus {
    /// The transaction was successful.
    Success {
        /// The Solana transaction hash.
        transaction_hash: String,
        /// The fee that was payed by the user.
        effective_transaction_fee: Option<Nat>,
    },
    /// The transaction failed.
    Failure {
        /// The Solana transaction hash.
        transaction_hash: String,
    },
}

/// Status of a withdrawal request.
#[derive(Clone, Eq, PartialEq, Hash, Debug, CandidType, Deserialize)]
pub enum WithdrawalStatus {
    /// Withdrawal request is not found.
    NotFound,

    /// Withdrawal request is waiting to be processed.
    Pending,

    /// Solana transaction was signed and is sent to the network.
    TxSent(SolTransaction),

    /// Solana transaction is finalized.
    TxFinalized(TxFinalizedStatus),
}

/// Information about the ckSOL minter canister.
#[derive(Clone, Debug, Eq, PartialEq, CandidType, Deserialize, Serialize)]
pub struct MinterInfo {
    /// Fee deducted from each deposit in the manual flow (SOL -> ckSOL).
    pub manual_deposit_fee: Lamport,
    /// Fee deducted from each deposit in the automated flow (SOL -> ckSOL).
    pub automated_deposit_fee: Lamport,
    /// Extra cycles charged per `process_deposit` call to offset deposit consolidation costs.
    pub deposit_consolidation_fee: u128,
    /// Minimum withdrawal amount in lamports.
    pub minimum_withdrawal_amount: Lamport,
    /// Minimum deposit amount in lamports.
    pub minimum_deposit_amount: Lamport,
    /// Fee deducted from each withdrawal (ckSOL -> SOL).
    pub withdrawal_fee: Lamport,
    /// Minimum cycles the caller must attach when calling `process_deposit`.
    pub process_deposit_required_cycles: u128,
    /// The minter's tracked SOL balance in lamports.
    pub balance: Lamport,
}
