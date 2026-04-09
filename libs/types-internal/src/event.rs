//! Candid-compatible event types for the ckSOL minter.

use crate::{InitArgs, UpgradeArgs};
use candid::CandidType;
use icrc_ledger_types::icrc1::account::Account;
use serde::Deserialize;
use sol_rpc_types::{Lamport, Signature, Slot};

/// A minter event that can be serialized to Candid.
#[derive(Clone, Debug, CandidType, Deserialize)]
pub struct Event {
    /// The canister time at which the minter generated this event.
    pub timestamp: u64,
    /// The event type.
    pub payload: EventType,
}

/// The type of a minter event.
#[derive(Clone, Debug, CandidType, Deserialize)]
pub enum EventType {
    /// The minter initialization event.
    /// Must be the first event in the log.
    Init(InitArgs),
    /// The minter upgraded with the specified arguments.
    Upgrade(UpgradeArgs),
    /// The minter discovered a Solana transaction that is a valid ckSOL
    /// deposit for the given account. ckSOL tokens have not yet been
    /// minted for this deposit.
    AcceptedDeposit {
        /// The signature of the Solana deposit transaction.
        signature: Signature,
        /// The account to which the minter should mint ckSOL.
        account: Account,
        /// The amount that was deposited.
        deposit_amount: Lamport,
        /// The amount of ckSOL tokens to mint for this deposit.
        /// This amount is generally lower than `deposit_amount` due
        /// to the deposit fee.
        amount_to_mint: Lamport,
    },
    /// The minter discovered a Solana transaction that is a valid ckSOL
    /// deposit, but it is unknown whether ckSOL tokens were minted for
    /// it or not, most likely because there was an unexpected panic in
    /// the callback.
    ///
    /// The deposit is quarantined to avoid any double minting and
    /// will not be further processed without manual intervention.
    QuarantinedDeposit {
        /// The signature of the Solana deposit transaction.
        signature: Signature,
        /// The account to which the minter should mint ckSOL.
        account: Account,
    },
    /// The minter minted ckSOL in response to a deposit.
    Minted {
        /// The signature of the Solana deposit transaction.
        signature: Signature,
        /// The account to which the minter minted ckSOL.
        account: Account,
        /// The transaction index on the ckSOL ledger.
        mint_block_index: u64,
    },
    /// The minter burned ckSOL for a withdrawal request.
    AcceptedWithdrawalRequest {
        /// The ledger account from which ckSOL was burned.
        account: Account,
        /// The destination Solana address.
        solana_address: [u8; 32],
        /// The burn transaction index on the ckSOL ledger.
        burn_block_index: u64,
        /// The total amount burned from the user (in lamports).
        amount_to_burn: Lamport,
        /// The net amount to transfer to the user (in lamports).
        withdrawal_amount: Lamport,
    },
    /// Submitted a Solana transaction.
    SubmittedTransaction {
        /// The signature of the Solana transaction.
        signature: Signature,
        /// The versioned transaction message.
        transaction: VersionedTransactionMessage,
        /// The signing accounts in signature order (fee payer first).
        signers: Vec<Account>,
        /// The slot of the blockhash used in the transaction.
        slot: Slot,
        /// The purpose of this transaction.
        purpose: TransactionPurpose,
    },
    /// A previously submitted transaction was resubmitted with a new signature.
    ResubmittedTransaction {
        /// The signature of the old transaction being replaced.
        old_signature: Signature,
        /// The signature of the new transaction.
        new_signature: Signature,
        /// The slot of the new blockhash used in the resubmitted transaction.
        new_slot: Slot,
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
}

/// The purpose of a submitted Solana transaction.
#[derive(Clone, Debug, CandidType, Deserialize)]
pub enum TransactionPurpose {
    /// Consolidate deposited funds into the minter's main account.
    ConsolidateDeposits {
        /// The mint indices of the deposits being consolidated.
        mint_indices: Vec<u64>,
    },
    /// Send withdrawals to users' Solana addresses.
    WithdrawSol {
        /// The burn transaction indices on the ckSOL ledger.
        burn_indices: Vec<u64>,
    },
}

/// A versioned Solana transaction message.
#[derive(Clone, Debug, CandidType, Deserialize)]
pub enum VersionedTransactionMessage {
    /// A legacy Solana transaction message, serialized with bincode.
    Legacy(Vec<u8>),
}

/// Arguments for the `get_events` endpoint.
#[derive(Clone, Debug, CandidType, Deserialize)]
pub struct GetEventsArgs {
    /// The index of the first event to return.
    pub start: u64,
    /// The maximum number of events to return.
    pub length: u64,
}

/// The result of a `get_events` call.
#[derive(Clone, Debug, CandidType, Deserialize)]
pub struct GetEventsResult {
    /// The events in the requested range.
    pub events: Vec<Event>,
    /// The total number of events in the log.
    pub total_event_count: u64,
}
