//! Candid types used by the Candid interface of the ckSOL minter.

#![forbid(unsafe_code)]
#![forbid(missing_docs)]

use candid::{CandidType, Nat, Principal};
use icrc_ledger_types::icrc1::account::{Account, Subaccount};
pub use memo::{BurnMemo, MAX_SERIALIZED_MEMO_BYTES, Memo, MintMemo, SplRefundKind};
use serde::{Deserialize, Serialize};
pub use sol_rpc_types::{Lamport, Pubkey as Address, Signature};
use thiserror::Error;

mod memo;

/// Index of a mint transaction on the ckSOL ledger.
pub type LedgerMintIndex = u64;

/// Index of a burn transaction on the ckSOL ledger.
pub type LedgerBurnIndex = u64;

/// Arguments for a request to the `get_deposit_address` ckSOL minter endpoint.
/// The result is the derived Solana owner address shared by SOL and supported SPL
/// deposits. SPL senders use a token transfer to this owner, whose associated token
/// account is derived for the mint.
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

/// Identifies a deposit queued by the `deposit_sol` ckSOL minter endpoint or by a balance scan
/// of the automatic flow, see `watch_deposit_address`.
///
/// A sequence number assigned when the deposit is queued.
pub type DepositSolId = u64;

/// The status of a deposit queued by the `deposit_sol` ckSOL minter endpoint or by a balance
/// scan of the automatic flow, see `watch_deposit_address`.
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
        /// transaction fee of the sweep and, for a deposit detected by the automatic
        /// flow, minus the automatic deposit fee.
        minted_amount: Lamport,
    },
    /// The sweep transaction failed, or expired without ever being seen on chain, so no
    /// ckSOL is owed. Calling `deposit_sol` again queues a new sweep of whatever balance
    /// the deposit address still holds. A deposit detected by the automatic flow also puts
    /// its deposit address back on watch for a new window, unless too many addresses are
    /// watched, so that `watch_deposit_address` reports it as `Watching` and the balance
    /// is swept again without a call; if too many addresses are watched, calling
    /// `watch_deposit_address` later registers the address again for free.
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
    /// mint results. Meanwhile the account stays in flight, so `deposit_sol` and
    /// `watch_deposit_address` keep rejecting it and a new deposit has to use a different
    /// subaccount.
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
    /// The latest deposit of the account is quarantined: either its sweep was finalized
    /// with an outcome that did not match the plan the minter submitted, so the deposit
    /// could not be credited safely, or its ckSOL mint could not be completed, see
    /// [`DepositSolStatus`]. The account stays rejected until a minter upgrade resolves
    /// the quarantined deposit; a new deposit has to use a different subaccount.
    #[error("The latest deposit {deposit_id} of this account is quarantined")]
    Quarantined {
        /// The identifier of the quarantined deposit.
        deposit_id: DepositSolId,
    },
}

/// Arguments for a request to the `watch_deposit_address` ckSOL minter endpoint.
#[derive(Clone, Eq, PartialEq, Debug, Default, CandidType, Deserialize, Serialize)]
pub struct WatchDepositAddressArgs {
    /// The subaccount of the caller to credit with the deposits detected on its deposit address.
    pub subaccount: Option<Subaccount>,
}

/// The successful result of a `watch_deposit_address` call.
#[derive(Clone, Eq, PartialEq, Debug, CandidType, Deserialize, Serialize)]
pub struct WatchDepositAddressOk {
    /// The deposit address of the caller's account.
    pub address: Address,
    /// The minimum deposit amount in force at the time of the call.
    pub minimum_deposit_amount: Lamport,
    /// The automatic deposit fee in force at the time of the call.
    pub automatic_deposit_fee: Lamport,
    /// Whether the account is watched or has a deposit in flight.
    pub status: WatchDepositAddressStatus,
    /// The id of the latest deposit queued for the account, if any, whatever stage it reached,
    /// to follow with `deposit_sol_status`. For a `Detected` status it is the id of the deposit
    /// in flight.
    pub latest_deposit_id: Option<DepositSolId>,
}

/// The status of an account registered with `watch_deposit_address`.
#[derive(Clone, Eq, PartialEq, Debug, CandidType, Deserialize, Serialize)]
pub enum WatchDepositAddressStatus {
    /// The deposit address is on the watchlist and scanned on the schedule.
    Watching {
        /// The end of the registration window, in nanoseconds since the epoch.
        valid_until: u64,
        /// The number of scans of the deposit address so far.
        scan_count: u32,
        /// The time of the last scan, in nanoseconds since the epoch, if any.
        last_scanned_at: Option<u64>,
        /// The time the next scan is due, in nanoseconds since the epoch, which may lie in the
        /// past until the tick that runs the scan, or `None` once the schedule leaves no scan
        /// before the window closes.
        next_scan_at: Option<u64>,
    },
    /// A deposit of the account is in flight: queued, swept, finalized or pending its mint.
    Detected {
        /// The identifier of the deposit in flight.
        deposit_id: DepositSolId,
    },
}

/// An error from the `watch_deposit_address` ckSOL minter endpoint.
#[derive(Debug, Clone, PartialEq, CandidType, Deserialize, Error)]
pub enum WatchDepositAddressError {
    /// The latest deposit of the account is quarantined, see
    /// [`DepositSolError::Quarantined`].
    #[error("The latest deposit {deposit_id} of this account is quarantined")]
    Quarantined {
        /// The identifier of the quarantined deposit.
        deposit_id: DepositSolId,
    },
    /// The watchlist is full of live registrations, try again later or use `deposit_sol`.
    #[error("The watchlist is full, try again later or use `deposit_sol`")]
    TooManyWatchedAddresses {
        /// The id of the latest deposit queued for the account, if any, to follow with
        /// `deposit_sol_status`. That deposit is minted or dropped, since an account with a
        /// deposit in flight needs no slot.
        latest_deposit_id: Option<DepositSolId>,
    },
    /// The minter experiences temporary issues, try the call again later.
    #[error("Transient error, try the call again later: {0}")]
    TemporarilyUnavailable(String),
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
pub struct WithdrawSolArgs {
    /// The subaccount to burn ckSOL from.
    pub from_subaccount: Option<Subaccount>,
    /// Amount to withdraw in Lamports.
    pub amount: Lamport,
    /// Address where to send Solana tokens.
    pub address: String,
}

/// The successful result of a withdrawal request.
#[derive(Clone, Eq, PartialEq, Debug, CandidType, Deserialize)]
pub struct WithdrawSolOk {
    /// The index of the burn block on the ckSOL ledger.
    pub block_index: LedgerBurnIndex,
}

/// Arguments for a request to the `withdraw_sol_status` ckSOL minter endpoint.
#[derive(Clone, Eq, PartialEq, Debug, CandidType, Deserialize)]
pub struct WithdrawSolStatusArgs {
    /// The burn block index returned by the `withdraw_sol` endpoint.
    pub block_index: LedgerBurnIndex,
}

/// The error result of a withdrawal request.
#[derive(Clone, Eq, PartialEq, Debug, CandidType, Deserialize)]
pub enum WithdrawSolError {
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
    },
    /// The transaction failed.
    Failure {
        /// The unique identifier (signature) of the Solana transaction.
        transaction_id: Signature,
    },
}

/// Status of a withdrawal request.
#[derive(Clone, Eq, PartialEq, Debug, CandidType, Deserialize)]
pub enum WithdrawSolStatus {
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

/// Arguments for a request to the `withdraw_spl` endpoint.
#[derive(Clone, Eq, PartialEq, Debug, CandidType, Deserialize, Serialize)]
pub struct WithdrawSplArgs {
    /// The Solana mint identifying the registered SPL token and its ckSPL ledger.
    pub mint: Address,
    /// The subaccount of the caller to burn ckSOL and ckSPL from.
    pub from_subaccount: Option<Subaccount>,
    /// The requested burn amount in the token's smallest units, not lamports.
    pub amount: u64,
    /// The destination wallet address on Solana, not a token account address.
    pub address: String,
}

/// The block index of an SPL withdrawal's fee burn on the ckSOL ledger.
///
/// Using the common fee ledger makes the identifier unique across all supported mints.
pub type WithdrawSplId = LedgerBurnIndex;

/// The acceptance receipt of an SPL withdrawal request.
///
/// Acceptance records the burn and queues the withdrawal; it does not mean that the
/// Solana transfer has been submitted or finalized.
#[derive(Clone, Eq, PartialEq, Debug, CandidType, Deserialize, Serialize)]
pub struct WithdrawSplOk {
    /// The ckSOL fee burn block index, also used to track the withdrawal.
    pub cksol_burn_block_index: WithdrawSplId,
    /// The amount burn block index on the ckSPL ledger selected by the request's mint.
    pub ckspl_burn_block_index: u64,
}

/// The error result of an SPL withdrawal request.
#[derive(Clone, Eq, PartialEq, Debug, CandidType, Deserialize)]
pub enum WithdrawSplError {
    /// The mint is not registered with the minter.
    TokenNotSupported {
        /// The requested Solana mint.
        mint: Address,
    },
    /// The token amount does not exceed the token's withdrawal fee, so nothing would be
    /// transferred. The minter enforces no other minimum.
    ValueTooSmall {
        /// The minimum amount in the token's smallest units.
        minimum_withdrawal_amount: u64,
        /// The requested amount in the token's smallest units.
        withdrawal_amount: u64,
    },
    /// Another withdrawal is being processed for this account.
    AlreadyProcessing,
    /// The destination cannot be parsed as a Solana address.
    MalformedAddress(String),
    /// The destination is not an allowed withdrawal address.
    InvalidDestination(String),
    /// The minter cannot currently accept the request.
    TemporarilyUnavailable(String),
    /// Burning the ckSOL withdrawal fee and rent reserve failed.
    CkSolLedgerError {
        /// The error from the ckSOL ledger.
        error: WithdrawSplLedgerError,
    },
    /// Burning ckSPL failed after the ckSOL fee was burned.
    CkSplLedgerError {
        /// The successful ckSOL fee burn index, also the withdrawal ID.
        cksol_burn_block_index: WithdrawSplId,
        /// The error from the ckSPL ledger.
        error: WithdrawSplLedgerError,
    },
}

/// A withdrawal burn error from either the ckSOL or ckSPL ledger.
#[derive(Clone, Eq, PartialEq, Debug, CandidType, Deserialize)]
pub enum WithdrawSplLedgerError {
    /// The account cannot cover the burn amount.
    InsufficientFunds {
        /// The ledger balance in the token's smallest units.
        balance: Nat,
        /// The amount the minter attempted to burn.
        failed_burn_amount: Nat,
        /// The ledger's configured token symbol.
        token_symbol: String,
        /// The ledger that rejected the burn.
        ledger_id: Principal,
    },
    /// The minter is not approved to burn the requested amount.
    InsufficientAllowance {
        /// The ledger allowance in the token's smallest units.
        allowance: Nat,
        /// The amount the minter attempted to burn.
        failed_burn_amount: Nat,
        /// The ledger's configured token symbol.
        token_symbol: String,
        /// The ledger that rejected the burn.
        ledger_id: Principal,
    },
    /// The requested amount is below the ledger's minimum burn amount.
    AmountTooLow {
        /// The minimum burn amount in the token's smallest units.
        minimum_burn_amount: Nat,
        /// The amount the minter attempted to burn.
        failed_burn_amount: Nat,
        /// The ledger's configured token symbol.
        token_symbol: String,
        /// The ledger that rejected the burn.
        ledger_id: Principal,
    },
    /// The ledger call failed or returned another error; the outcome may be unresolved.
    TemporarilyUnavailable(String),
}

/// Arguments for a request to the `withdraw_spl_status` endpoint.
#[derive(Clone, Eq, PartialEq, Debug, CandidType, Deserialize)]
pub struct WithdrawSplStatusArgs {
    /// The ckSOL fee burn block index returned by the `withdraw_spl` endpoint.
    pub cksol_burn_block_index: WithdrawSplId,
}

/// Status of an SPL withdrawal request.
#[derive(Clone, Eq, PartialEq, Debug, CandidType, Deserialize)]
pub enum WithdrawSplStatus {
    /// Withdrawal request is not found.
    NotFound,
    /// Withdrawal request is waiting to be processed.
    Pending,
    /// The token burn failed after the fee burn; the ckSOL fee refund is queued.
    FeeReimbursementPending {
        /// The ckSOL amount to mint back, in lamports.
        reimbursed_amount: Lamport,
    },
    /// The ckSOL fee was minted back.
    FeeReimbursed {
        /// The ckSOL amount minted back, in lamports.
        reimbursed_amount: Lamport,
        /// The mint block index on the ckSOL ledger.
        mint_block_index: u64,
    },
    /// The ckSOL fee refund cannot be retried safely and requires manual resolution.
    FeeReimbursementQuarantined {
        /// The ckSOL amount that was to be minted back, in lamports.
        reimbursed_amount: Lamport,
    },
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
    /// The fee in lamports deducted from the minted amount of a deposit detected by the
    /// automatic flow, once configured.
    pub automatic_deposit_fee: Option<Lamport>,
    /// The sum in lamports of the automatic deposit fees collected so far.
    pub collected_automatic_deposit_fees: Option<Lamport>,
    /// Registered SPL tokens in mint-address order; empty when none are registered.
    /// Absent in responses from older minter versions.
    pub supported_spl_tokens: Option<Vec<SplTokenInfo>>,
    /// Cycles to attach for a new manual SPL deposit: the source-read budget plus consolidation.
    pub deposit_spl_required_cycles: Option<u128>,
    /// Cycle fee charged only when a new SPL deposit is queued; excludes the source-read cost.
    pub spl_deposit_consolidation_fee: Option<u128>,
    /// The SPL deposit fees collected so far, with one entry per registered token in
    /// mint-address order. Absent in responses from older minter versions.
    pub collected_spl_deposit_fees: Option<Vec<CollectedSplDepositFees>>,
}

/// An entry of the list returned by the [ICRC-10](https://github.com/dfinity/ICRC/blob/main/ICRCs/ICRC-10/ICRC-10.md)
/// `icrc10_supported_standards` endpoint.
#[derive(Clone, Debug, Eq, PartialEq, CandidType, Deserialize, Serialize)]
pub struct StandardRecord {
    /// The name of the standard, e.g. "ICRC-21".
    pub name: String,
    /// The URL of the specification of the standard.
    pub url: String,
}

/// Registry and deposit settings for one supported SPL token.
#[derive(Clone, Debug, Eq, PartialEq, CandidType, Deserialize, Serialize)]
pub struct SplTokenInfo {
    /// Solana mint address.
    pub mint: Address,
    /// Classic SPL Token program address.
    pub token_program: Address,
    /// Token and ledger decimals.
    pub decimals: u8,
    /// The ckSPL ledger canister.
    pub ledger_id: Principal,
    /// Optional index canister.
    pub index_id: Option<Principal>,
    /// The ckSPL ledger symbol.
    pub symbol: String,
    /// Minimum net deposit in the token's smallest units.
    pub minimum_deposit_amount: u64,
    /// Fixed deposit fee in the token's smallest units.
    pub deposit_fee: u64,
    /// Whether new deposit requests are paused.
    pub deposits_paused: bool,
    /// Withdrawal fee in the token's smallest units. A withdrawal of `amount` burns `amount`
    /// and transfers `amount - withdrawal_fee_token`. The minter enforces no minimum amount; the
    /// token ledger rejects a burn below its own minimum.
    pub withdrawal_fee_token: u64,
    /// The smallest amount that a withdrawal request can have, in the token's smallest
    /// units: `withdrawal_fee_token + 1`. It is derived, not a setting.
    pub minimum_withdrawal_amount: u64,
    /// The fee in lamports of ckSOL that a withdrawal of this token burns and keeps.
    pub withdrawal_fee_cksol: Lamport,
    /// The lamports of ckSOL that a withdrawal burns for the rent of the destination token
    /// account. The part that is not spent is refunded in ckSOL.
    pub refundable_rent: Lamport,
    /// The ckSOL that a withdrawal burns: `withdrawal_fee_cksol + refundable_rent`. The
    /// allowance on the ckSOL ledger must cover it.
    pub total_withdrawal_fee_cksol: Lamport,
}

/// The deposit fees collected so far for one supported SPL token.
#[derive(Clone, Debug, Eq, PartialEq, CandidType, Deserialize, Serialize)]
pub struct CollectedSplDepositFees {
    /// Solana mint address.
    pub mint: Address,
    /// The sum of the collected fees in the token's smallest units.
    pub amount: u128,
}

/// Unbacked SOL available for minter operating costs and its ckSOL funding account.
#[derive(Clone, Debug, Eq, PartialEq, CandidType, Deserialize, Serialize)]
pub struct OperatingBalance {
    /// Lamports credited by completed refill burns; excludes unresolved burns.
    pub balance: u64,
    /// Donate ckSOL here to fund automatic operating-SOL refills.
    pub top_up_account: Account,
}

/// Sequence number of a manually queued SPL deposit, independent of SOL deposit IDs.
pub type DepositSplId = u64;

/// Arguments for a manual SPL deposit. The caller may sponsor another owner's deposit.
#[derive(Clone, Eq, PartialEq, Debug, CandidType, Deserialize, Serialize)]
pub struct DepositSplArgs {
    /// Beneficiary principal; defaults to the caller.
    pub owner: Option<Principal>,
    /// Beneficiary subaccount; defaults to the all-zero subaccount.
    pub subaccount: Option<Subaccount>,
    /// Registered classic SPL mint.
    pub mint: Address,
}

/// The status of a deposit queued by the `deposit_spl` ckSOL minter endpoint.
#[derive(Clone, Eq, PartialEq, Debug, CandidType, Deserialize, Serialize)]
pub enum DepositSplStatus {
    /// No deposit with this identifier was queued.
    NotFound,
    /// The token account is queued for a sweep to the minter's treasury token account.
    Queued {
        /// The recorded balance that will be swept, in the mint's smallest units.
        balance: u64,
        /// The fee recorded at acceptance, in the mint's smallest units.
        fee: u64,
    },
    /// A Solana transaction sweeping the token account to the treasury has been submitted
    /// but its outcome is not settled yet.
    Swept {
        /// The signature of the sweep transaction.
        signature: Signature,
    },
    /// The sweep transaction was finalized successfully, so the tokens reached the
    /// treasury, but the ckSPL mint has not landed yet.
    Finalized {
        /// The signature of the sweep transaction.
        signature: Signature,
    },
    /// The minter minted ckSPL for the deposit on its token ledger.
    Minted {
        /// The mint transaction index on the token ledger.
        block_index: LedgerMintIndex,
        /// The minted amount: the swept balance minus the fee, in the mint's smallest units.
        minted_amount: u64,
    },
    /// The sweep transaction failed, or expired without ever being seen on chain, so no
    /// ckSPL is owed. Calling `deposit_spl` again queues a new sweep of whatever balance
    /// the token account holds.
    Dropped {
        /// The signature of the sweep transaction.
        signature: Signature,
    },
    /// Either the outcome of the sweep did not match its plan, so no ckSPL was minted, or
    /// the mint could not be retried safely, so the mint outcome requires manual
    /// verification. Releasing or crediting the deposit requires a minter upgrade.
    /// Meanwhile the account and mint pair stays in flight, so `deposit_spl` keeps
    /// rejecting it and a new deposit has to use a different subaccount.
    Quarantined {
        /// The signature of the sweep transaction.
        signature: Signature,
    },
}

/// A manual SPL deposit could not be queued.
#[derive(Debug, Clone, PartialEq, CandidType, Deserialize, Error)]
pub enum DepositSplError {
    /// Another request for the same account and mint is awaiting its balance read.
    #[error("There is already a concurrent SPL deposit request for this account and mint")]
    AlreadyProcessing,
    /// Attach the reported cycle budget before making the source-account RPC call.
    #[error(transparent)]
    InsufficientCycles(#[from] InsufficientCyclesError),
    /// Source tokens do not cover the minimum net deposit plus captured token fee.
    #[error("Balance {balance} is below the minimum required balance {minimum_balance}")]
    ValueTooSmall {
        /// Gross source token balance.
        balance: u64,
        /// Required gross token balance, including the token fee.
        minimum_balance: u64,
    },
    /// The mint has no registry entry.
    #[error("Unsupported SPL token: {mint}")]
    UnsupportedToken {
        /// Requested mint address.
        mint: Address,
    },
    /// New deposits are paused for this mint.
    #[error("SPL deposits are paused for token {mint}")]
    TokenPaused {
        /// Requested mint address.
        mint: Address,
    },
    /// Wrong program, token owner, mint, state or account encoding.
    #[error("Invalid SPL token account: {0}")]
    InvalidTokenAccount(String),
    /// The latest deposit of this account and mint is quarantined: the outcome of its sweep
    /// did not match the plan, or its mint could not be retried safely. The pair stays
    /// rejected until a minter upgrade resolves the deposit; a new deposit has to use a
    /// different subaccount.
    #[error("The latest deposit {deposit_id} of this account and mint is quarantined")]
    Quarantined {
        /// The identifier of the quarantined deposit.
        deposit_id: DepositSplId,
    },
    /// A key or RPC result is unavailable, or concurrent request capacity was reached.
    #[error("SPL deposit is temporarily unavailable: {0}")]
    TemporarilyUnavailable(String),
}
