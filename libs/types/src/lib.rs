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

/// Index of a mint transaction on the ckSOL ledger.
pub type LedgerMintIndex = u64;

/// Index of a burn transaction on the ckSOL ledger.
pub type LedgerBurnIndex = u64;

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

/// Arguments for a request to the `deposit_sol` ckSOL minter endpoint.
#[derive(Clone, Eq, PartialEq, Debug, Default, CandidType, Deserialize, Serialize)]
pub struct DepositSolArgs {
    /// The principal to credit with the deposit.
    ///
    /// If not set, defaults to the caller's principal.
    /// The resolved owner must be a non-anonymous principal.
    pub owner: Option<Principal>,
    /// The subaccount to credit with the deposit.
    pub subaccount: Option<Subaccount>,
}

impl From<Account> for DepositSolArgs {
    fn from(account: Account) -> Self {
        Self {
            owner: Some(account.owner),
            subaccount: account.subaccount,
        }
    }
}

/// Identifies a deposit queued by the `deposit_sol` ckSOL minter endpoint.
///
/// A sequence number assigned when the deposit is queued.
pub type DepositSolId = u64;

/// The status of a deposit queued by the `deposit_sol` ckSOL minter endpoint.
#[derive(Clone, Eq, PartialEq, Debug, CandidType, Deserialize, Serialize)]
pub enum DepositSolStatus {
    /// No deposit with this identifier was queued.
    NotFound,
    /// The deposit address is queued for a sweep to the minter's main account.
    Queued {
        /// The amount that will be swept from the deposit address.
        sweepable_amount: Lamport,
    },
    /// A Solana transaction sweeping the deposit address to the minter's main account
    /// has been submitted but is not yet finalized.
    Swept {
        /// The signature of the sweep transaction.
        signature: Signature,
    },
    /// The sweep transaction was finalized successfully, so the deposited SOL reached
    /// the minter's main account, but the ckSOL mint has not landed yet.
    Finalized {
        /// The signature of the sweep transaction.
        signature: Signature,
    },
    /// The minter minted ckSOL for the deposit on the ledger.
    Minted {
        /// The mint transaction index on the ckSOL ledger.
        block_index: LedgerMintIndex,
        /// The minted amount: the swept amount minus the deposit's share of the
        /// transaction fee of the sweep.
        minted_amount: Lamport,
    },
    /// The sweep transaction failed, or expired without ever being seen on chain, so no
    /// ckSOL is owed. Calling `deposit_sol` again queues a new sweep of whatever balance
    /// the deposit address still holds.
    Dropped {
        /// The signature of the sweep transaction.
        signature: Signature,
    },
    /// The minter stopped processing the deposit: either the finalized sweep's outcome
    /// did not match the plan the minter submitted it with, so the amount to credit
    /// cannot be determined safely and no ckSOL was minted, or the ckSOL mint of the
    /// credited deposit could not be completed, in which case the mint may nevertheless
    /// have landed on the ledger. This is not expected to happen and resolving it
    /// requires a minter upgrade: before minting by hand, search the ledger for a mint
    /// whose memo carries the sweep signature and the deposit id, otherwise a double
    /// mint results. Meanwhile the account stays in flight, so `deposit_sol` keeps
    /// rejecting it and a new deposit has to use a different subaccount.
    Quarantined {
        /// The signature of the sweep transaction.
        signature: Signature,
    },
}

/// An error from the `deposit_sol` ckSOL minter endpoint.
#[derive(Debug, Clone, PartialEq, CandidType, Deserialize, Error)]
pub enum DepositSolError {
    /// Insufficient cycles attached by the caller to complete the `deposit_sol` call.
    #[error(transparent)]
    InsufficientCycles(#[from] InsufficientCyclesError),
    /// The minter experiences temporary issues, try the call again later.
    #[error("Transient error, try the call again later: {0}")]
    TemporarilyUnavailable(String),
    /// There is already a concurrent `deposit_sol` invocation for the same account.
    #[error("There is already a concurrent `deposit_sol` invocation for the same account")]
    AlreadyProcessing,
    /// The balance of the deposit address is below the minimum deposit amount.
    ///
    /// The minimum deposit amount applies to the balance of the deposit address and includes
    /// the rent exemption threshold, so a deposit of exactly the minimum is accepted.
    #[error(
        "Insufficient deposit address balance: expected at least {minimum_deposit_amount} lamports, but got {balance} lamports"
    )]
    ValueTooSmall {
        /// The balance of the deposit address.
        balance: Lamport,
        /// The minimum deposit amount for the deposit to be queued.
        minimum_deposit_amount: Lamport,
    },
    /// The latest deposit of the account is quarantined: its sweep was finalized, but the
    /// outcome did not match the plan the minter submitted, so the deposit could not be
    /// credited safely. The account stays rejected until a minter upgrade resolves the
    /// quarantined deposit; a new deposit has to use a different subaccount.
    #[error("The latest deposit {deposit_id} of this account is quarantined")]
    Quarantined {
        /// The identifier of the quarantined deposit.
        deposit_id: DepositSolId,
    },
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
    pub amount: Lamport,
    /// Address where to send Solana tokens.
    pub address: String,
}

/// The successful result of a withdrawal request.
#[derive(Clone, Eq, PartialEq, Debug, CandidType, Deserialize)]
pub struct WithdrawalOk {
    /// The index of the burn block on the ckSOL ledger.
    pub block_index: LedgerBurnIndex,
}

/// Arguments for a request to the `withdrawal_status` ckSOL minter endpoint.
#[derive(Clone, Eq, PartialEq, Debug, CandidType, Deserialize)]
pub struct WithdrawalStatusArgs {
    /// The burn block index returned by the `withdraw_sol` endpoint.
    pub block_index: LedgerBurnIndex,
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
    /// The destination address is not a valid withdrawal destination.
    /// The payload contains a human-readable message explaining why the destination was rejected.
    InvalidDestination(String),
    /// The withdrawal account does not hold the requested ckSOL amount.
    InsufficientFunds {
        /// The current balance of the withdrawal account.
        balance: Lamport,
    },
    /// The minter is not approved to transfer the requested amount.
    InsufficientAllowance {
        /// The current allowance for the minter.
        allowance: Lamport,
    },
    /// There are too many concurrent requests, retry later.
    TemporarilyUnavailable(String),
}

/// Status of a finalized transaction.
#[derive(Clone, Eq, PartialEq, Debug, CandidType, Deserialize)]
pub enum TxFinalizedStatus {
    /// The transaction was successful.
    Success {
        /// The unique identifier (signature) of the Solana transaction.
        transaction_id: Signature,
        /// The fee that was paid by the user.
        effective_transaction_fee: Option<Nat>,
    },
    /// The transaction failed.
    Failure {
        /// The unique identifier (signature) of the Solana transaction.
        transaction_id: Signature,
    },
}

/// Status of a withdrawal request.
#[derive(Clone, Eq, PartialEq, Debug, CandidType, Deserialize)]
pub enum WithdrawalStatus {
    /// Withdrawal request is not found.
    NotFound,
    /// Withdrawal request is waiting to be processed.
    Pending,
    /// Solana transaction was signed and is sent to the network.
    TxSent {
        /// The unique identifier (signature) of the Solana transaction.
        transaction_id: Signature,
    },
    /// Solana transaction is finalized.
    TxFinalized(TxFinalizedStatus),
}

/// Information about the ckSOL minter canister.
#[derive(Clone, Debug, Eq, PartialEq, CandidType, Deserialize, Serialize)]
pub struct MinterInfo {
    /// Extra cycles charged per `deposit_sol` call to offset the cost of the sweep.
    pub deposit_sol_fee: u128,
    /// Minimum withdrawal amount in lamports.
    pub minimum_withdrawal_amount: Lamport,
    /// Minimum deposit amount in lamports.
    pub minimum_deposit_amount: Lamport,
    /// Fee deducted from each withdrawal (ckSOL -> SOL).
    pub withdrawal_fee: Lamport,
    /// Minimum cycles the caller must attach when calling `deposit_sol`.
    pub deposit_sol_required_cycles: u128,
    /// The minter's tracked SOL balance in lamports.
    pub balance: Lamport,
    /// The minter's main Solana address, available once its Schnorr public key
    /// has been fetched for the first time, shortly after installation.
    pub minter_address: Option<String>,
    /// The base58 addresses of the durable nonce accounts reserved for withdrawal transactions.
    pub nonce_accounts: Vec<String>,
}
