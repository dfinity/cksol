//! Candid-compatible event types for the ckSOL minter.

use crate::{InitArgs, UpgradeArgs};
use candid::CandidType;
use icrc_ledger_types::icrc1::account::Account;
use serde::Deserialize;
use sol_rpc_types::{Lamport, Pubkey as Address, Signature};

/// A minter event that can be serialized to Candid.
#[derive(Clone, Debug, PartialEq, CandidType, Deserialize)]
pub struct Event {
    /// The canister time at which the minter generated this event.
    pub timestamp: u64,
    /// The event type.
    pub payload: EventType,
}

/// The type of a minter event.
#[derive(Clone, Debug, PartialEq, CandidType, Deserialize)]
pub enum EventType {
    /// The minter initialization event.
    /// Must be the first event in the log.
    Init(InitArgs),
    /// The minter upgraded with the specified arguments.
    Upgrade(UpgradeArgs),
    /// The minter burned ckSOL for a withdrawal request.
    AcceptedWithdrawalRequest {
        /// The ledger account from which ckSOL was burned.
        account: Account,
        /// The destination Solana address.
        solana_address: Address,
        /// The burn transaction index on the ckSOL ledger.
        burn_block_index: u64,
        /// The total amount burned from the user (in lamports).
        burned_amount: Lamport,
        /// The net amount to transfer to the user (in lamports).
        amount_to_transfer: Lamport,
    },
    /// Submitted a Solana transaction.
    SubmittedTransaction {
        /// The signature of the Solana transaction.
        signature: Signature,
        /// The versioned transaction message.
        transaction: VersionedTransactionMessage,
        /// The signers in signature order (fee payer first).
        signers: Vec<Signer>,
        /// The purpose of this transaction.
        purpose: TransactionPurpose,
        /// The block height of the block whose blockhash the transaction uses.
        block_height: u64,
    },
    /// A previously submitted transaction was resubmitted with a new signature.
    ResubmittedTransaction {
        /// The signature of the old transaction being replaced.
        old_signature: Signature,
        /// The signature of the new transaction.
        new_signature: Signature,
        /// The block height of the new blockhash used in the resubmitted transaction.
        new_block_height: u64,
    },
    /// A previously submitted Solana transaction has been finalized successfully.
    SucceededTransaction {
        /// The signature of the succeeded Solana transaction.
        signature: Signature,
    },
    /// A previously submitted Solana transaction has failed.
    FailedTransaction {
        /// The signature of the failed Solana transaction.
        signature: Signature,
    },
    /// A previously submitted Solana transaction has an expired blockhash
    /// and a null on-chain status, meaning it will never be executed.
    /// A withdrawal transaction is marked for resubmission;
    /// the deposits of a sweep transaction are dropped instead.
    ExpiredTransaction {
        /// The signature of the expired Solana transaction.
        signature: Signature,
    },
    /// A user queued the deposit address of an account for a sweep via `deposit_sol`.
    QueuedDeposit {
        /// The identifier of the queued deposit.
        deposit_id: u64,
        /// The account to which the minter should mint ckSOL once the sweep is finalized.
        account: Account,
        /// The deposit address derived from the account.
        address: Address,
        /// The balance of the deposit address when the deposit was queued.
        balance: Lamport,
    },
    /// The minter read the amount that the finalized sweep transaction moved to its
    /// main account and enqueued a pending mint for each deposit of that sweep.
    CreditedSweep {
        /// The signature of the finalized sweep transaction.
        signature: Signature,
        /// The increase of the main account balance reported by the transaction metadata.
        amount_received: Lamport,
        /// The mint enqueued for each deposit of the sweep.
        mints: Vec<CreditedDeposit>,
    },
    /// The outcome of a finalized sweep transaction did not match the plan the minter
    /// submitted it with, so the amount to credit cannot be determined safely.
    ///
    /// The deposits are quarantined to avoid any double minting and will not be further
    /// processed without manual intervention.
    QuarantinedSweep {
        /// The signature of the finalized sweep transaction.
        signature: Signature,
    },
    /// The minter minted ckSOL on the ledger for a swept deposit whose sweep
    /// was credited.
    MintedSweptDeposit {
        /// The identifier of the minted deposit.
        deposit_id: u64,
        /// The mint transaction index on the ckSOL ledger.
        mint_block_index: u64,
    },
    /// The pending mint of a swept deposit cannot be retried: either it became
    /// older than the 24-hour deduplication window of the ckSOL ledger, or the
    /// ledger definitively rejected it. Retrying the transfer with the same
    /// arguments fails forever, and fresh arguments could double mint.
    ///
    /// The deposit is quarantined to avoid any double minting and will not be
    /// further processed without manual intervention.
    ///
    /// If the minter was down past the deduplication window, the underlying
    /// transfer may nevertheless have landed on the ledger. Manual resolution
    /// must therefore first search the ledger for a mint whose memo carries the
    /// sweep signature before crediting by hand, otherwise a double mint
    /// results.
    QuarantinedPendingMint {
        /// The identifier of the deposit whose pending mint was quarantined.
        deposit_id: u64,
    },
}

/// The mint enqueued for one deposit of a `CreditedSweep` event.
#[derive(Clone, Copy, Debug, PartialEq, CandidType, Deserialize)]
pub struct CreditedDeposit {
    /// The identifier of the deposit.
    pub deposit_id: u64,
    /// The sweepable amount minus the deposit's share of the transaction fee of the sweep.
    pub amount_to_mint: Lamport,
}

/// The key that produced one signature of a submitted Solana transaction.
#[derive(Clone, Debug, PartialEq, CandidType, Deserialize)]
pub enum Signer {
    /// The minter itself, signing with the master key that controls
    /// the minter's main address.
    Minter,
    /// A minter-controlled account, signing with the key derived
    /// for its deposit address.
    Account(Account),
}

/// The purpose of a submitted Solana transaction.
#[derive(Clone, Debug, PartialEq, CandidType, Deserialize)]
pub enum TransactionPurpose {
    /// Send withdrawals to users' Solana addresses.
    WithdrawSol {
        /// The burn transaction indices on the ckSOL ledger.
        burn_indices: Vec<u64>,
    },
    /// Sweep the deposit addresses of deposits queued by `deposit_sol` into the minter's main account.
    SweepDeposits {
        /// The ids of the swept deposits.
        deposit_ids: Vec<u64>,
    },
}

/// A versioned Solana transaction message.
#[derive(Clone, Debug, PartialEq, CandidType, Deserialize)]
pub enum VersionedTransactionMessage {
    /// A legacy Solana transaction message, serialized with bincode.
    Legacy(Vec<u8>),
}

/// Arguments for the `get_events` endpoint.
#[derive(Clone, Debug, PartialEq, CandidType, Deserialize)]
pub struct GetEventsArgs {
    /// The index of the first event to return.
    pub start: u64,
    /// The maximum number of events to return.
    pub length: u64,
}

/// The result of a `get_events` call.
#[derive(Clone, Debug, PartialEq, CandidType, Deserialize)]
pub struct GetEventsResult {
    /// The events in the requested range.
    pub events: Vec<Event>,
    /// The total number of events in the log.
    pub total_event_count: u64,
}
