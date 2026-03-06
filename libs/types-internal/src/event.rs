//! Candid-compatible event types for the ckSOL minter.

use crate::{InitArgs, UpgradeArgs};
use candid::CandidType;
use icrc_ledger_types::icrc1::account::Account;
use serde::Deserialize;
use sol_rpc_types::{Lamport, Signature};

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
    AcceptedWithdrawSolRequest {
        /// The ledger account from which ckSOL was burned.
        account: Account,
        /// The destination Solana address.
        solana_address: [u8; 32],
        /// The burn transaction index on the ckSOL ledger.
        burn_block_index: u64,
        /// The total amount burned from the user (in lamports).
        withdrawal_amount: Lamport,
        /// The fee retained by the minter (in lamports).
        withdrawal_fee: Lamport,
    },
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
