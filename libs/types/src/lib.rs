//! Candid types used by the Candid interface of the ckSOL minter.

#![forbid(unsafe_code)]
#![forbid(missing_docs)]

use candid::{CandidType, Nat, Principal};
use icrc_ledger_types::icrc1::account::Subaccount;
use serde::{Deserialize, Serialize};
use sol_rpc_types::Lamport;
pub use sol_rpc_types::{Pubkey as Address, Signature};
use thiserror::Error;

/// The outcome of processing a Solana deposit transaction.
#[derive(Clone, Eq, PartialEq, Debug, CandidType, Deserialize, Serialize)]
pub enum DepositStatus {
    /// The transaction is a valid deposit, but the minter failed to mint ckSOL on the ledger.
    Checked(Signature),
    /// The minter accepted the deposit and minted ckSOL tokens on the ledger.
    Minted {
        /// The MINT transaction index on the ledger.
        block_index: u64,
        /// The minted amount (deposit amount minus fees).
        minted_amount: Lamport,
        /// The UTXO that caused the balance update.
        signature: Signature,
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

/// Arguments for a request to the `update_balance` ckSOL minter endpoint.
#[derive(Clone, Eq, PartialEq, Debug, CandidType, Deserialize, Serialize)]
pub struct UpdateBalanceArgs {
    /// If provided, update the balance for this principal.
    ///
    /// If not set, defaults to the caller's principal.
    /// The resolved owner must be a non-anonymous principal.
    pub owner: Option<Principal>,
    /// The subaccount for which to update the balance.
    pub subaccount: Option<Subaccount>,
    /// Signature of the deposit transaction.
    pub signature: Signature,
}

/// An error from the `update_balance` ckSOL minter endpoint.
#[derive(Debug, Clone, PartialEq, CandidType, Deserialize, Error)]
pub enum UpdateBalanceError {
    /// The minter experiences temporary issues, try the call again later.
    #[error("Transient error, try the call again later: {0}")]
    TemporarilyUnavailable(String),
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
    #[error("The deposit amount does not cover the deposit fee")]
    ValueTooSmall,
}

/// Arguments for a request to the `retrieve_sol` ckSOL minter endpoint.
#[derive(Clone, Eq, PartialEq, Debug, Default, CandidType, Deserialize, Serialize)]
pub struct RetrieveSolArgs {
    /// The subaccount to burn ckSOL from.
    pub from_subaccount: Option<Subaccount>,

    /// Amount to retrieve in Lamports.
    pub amount: u64,

    /// Address where to send Solana tokens.
    pub address: String,
}

/// The successful result of calling the `retrieve_sol` endpoint.
#[derive(Clone, Eq, PartialEq, Debug, CandidType, Deserialize)]
pub struct RetrieveSolOk {
    /// The index of the burn block on the ckSOL ledger.
    pub block_index: u64,
}

/// The error result of calling the `retrieve_sol` endpoint.
#[derive(Clone, Eq, PartialEq, Debug, CandidType, Deserialize)]
pub enum RetrieveSolError {
    /// There is another request for this principal.
    AlreadyProcessing,

    /// The withdrawal amount is too low.
    AmountTooLow(u64),

    /// The Solana address is not valid.
    MalformedAddress(String),

    /// The withdrawal account does not hold the requested ckSOL amount.
    InsufficientFunds {
        /// The current balance of the withdrawal account.
        balance: u64,
    },

    /// There are too many concurrent requests, retry later.
    TemporarilyUnavailable(String),

    /// A generic error reserved for future extensions.
    GenericError {
        /// Generic error message.
        error_message: String,
        /// Generic error code.
        error_code: u64,
    },
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
}

/// Retrieve the status of a withdrawal request.
#[derive(Clone, Eq, PartialEq, Hash, Debug, CandidType, Deserialize)]
pub enum RetrieveSolStatus {
    /// Withdrawal request is not found.
    NotFound,

    /// Withdrawal request is waiting to be processed.
    Pending,

    /// Transaction fees were estimated and a Solana transaction was created.
    /// Transaction is not signed yet.
    TxCreated,

    /// Solana transaction was signed and is sent to the network.
    TxSent(SolTransaction),

    /// Solana transaction is finalized.
    TxFinalized(TxFinalizedStatus),
}

/// Information about the ckSOL minter canister.
#[derive(Clone, Debug, Eq, PartialEq, CandidType, Deserialize, Serialize)]
pub struct MinterInfo {
    /// Fee deducted from each deposit (SOL -> ckSOL).
    pub deposit_fee: Lamport,
}
