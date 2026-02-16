//! Candid types used by the Candid interface of the ckSOL minter.

#![forbid(unsafe_code)]
#![forbid(missing_docs)]

use candid::{CandidType, Nat, Principal};
use icrc_ledger_types::icrc1::account::Subaccount;
use serde::{Deserialize, Serialize};
pub use sol_rpc_types::Pubkey as Address;

/// Arguments for a request to the `getDepositAddress` ckSOL minter endpoint.
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
    /// The index of the burn block on the ckSOL ledger
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
    /// Transaction failed and will be reimbursed.
    PendingReimbursement(SolTransaction),
    /// Transaction failed, user got reimbursed.
    Reimbursed {
        /// The Solana transaction hash.
        transaction_hash: String,
        /// The amount in Lamports that was returned to the user.
        reimbursed_amount: Nat,
        /// The ckSOL ledger block containing the reimbursment transaction.
        reimbursed_in_block: Nat,
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
