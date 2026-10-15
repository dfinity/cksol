use std::{num::NonZeroUsize, time::Duration};

/// Maximum number of concurrent calls to the SOL RPC canister.
pub const MAX_CONCURRENT_RPC_CALLS: usize = 10;

/// Maximum number of concurrent threshold signing requests to the management canister
/// in a single round of a timer.
pub const MAX_CONCURRENT_SIGNATURES: usize = 10;

/// Maximum number of attempts to fetch a recent block, each attempt consisting of
/// at most one `getSlot` and one `getBlock` call.
pub const GET_RECENT_BLOCK_MAX_TRIES: NonZeroUsize =
    NonZeroUsize::new(3).expect("BUG: the maximum number of tries must be non-zero");

/// Interval of the timer sweeping queued deposits, the same as withdrawal processing.
pub const SWEEP_DEPOSITS_DELAY: Duration = Duration::from_mins(1);

/// Delay before retrying the minter public key fetch after it trapped. Deposits and
/// transfer creation stay unavailable until the key is recorded, so the retry is quick.
pub const MINTER_PUBLIC_KEY_RETRY_DELAY: Duration = Duration::from_secs(5);

/// Interval of the timer minting the pending mints of credited sweeps.
pub const PROCESS_PENDING_MINTS_DELAY: Duration = Duration::from_mins(1);

/// Maximum number of pending mints sent to the ckSOL ledger, one after the other,
/// in a single round of the minting timer.
pub const MAX_PENDING_MINTS_PER_ROUND: usize = 10;

/// The window within which the ckSOL ledger deduplicates transfers by their
/// `created_at_time`, matching `TRANSACTION_WINDOW` of the ICRC-1 ledger:
/// <https://github.com/dfinity/ic/blob/d3b3351fa343a893aea2cfa8d39ac6b0d507b477/rs/ledger_suite/icrc1/ledger/src/lib.rs#L65>
///
/// Retrying a pending mint beyond this window is no longer deduplicated, so a
/// pending mint older than that is quarantined instead of retried.
pub const LEDGER_DEDUPLICATION_WINDOW: Duration = Duration::from_hours(24);

/// Cycles to attach for `getTransaction` RPC calls.
///
/// The SOL RPC canister charges about 3.2B cycles for a `getTransaction`
/// request with the default 3-out-of-4 provider consensus and its default
/// response size estimate of 10 KiB, which fits a full sweep or withdrawal
/// transaction. The attached amount leaves a wide margin for provider or price
/// changes and for the SOL RPC canister doubling the response size estimate
/// after an oversized response; the unused part is refunded.
pub const GET_TRANSACTION_CYCLES: u128 = 50_000_000_000;

/// Cycles to attach for `getBalance` RPC calls.
///
/// The SOL RPC canister charges about 2.1B cycles for a `getBalance` request
/// with the default 3-out-of-4 provider consensus, comparable to `getSlot`
/// given the similarly small response (see section 3.3.2 of the design). The
/// attached amount leaves a wide margin for provider or price changes; the
/// unused part is refunded and never charged to the caller of `deposit_sol`.
pub const GET_BALANCE_CYCLES: u128 = 10_000_000_000;

/// Maximum number of addresses per `getMultipleAccounts` RPC call.
/// See <https://solana.com/docs/rpc/http/getmultipleaccounts>.
pub const MAX_ADDRESSES_PER_GET_MULTIPLE_ACCOUNTS: usize = 100;

/// Cycles to attach for a `getMultipleAccounts` RPC call reading one address,
/// on top of [`GET_MULTIPLE_ACCOUNTS_CYCLES_PER_ADDRESS`] for every address.
///
/// The SOL RPC canister prices the call like `getBalance` for a single
/// address: about 2.3B cycles with the default 3-out-of-4 provider consensus.
/// The attached amount, which the SOL RPC client attaches by default too,
/// leaves a wide margin for provider or price changes; the unused part is
/// refunded.
pub const GET_MULTIPLE_ACCOUNTS_BASE_CYCLES: u128 = 10_000_000_000;

/// Cycles to attach per address of a `getMultipleAccounts` RPC call, covering
/// the roughly 150 bytes of response an address without data adds, which
/// the SOL RPC canister charges about 22M cycles for (see section 3.3.2 of
/// the design); the unused part is refunded.
pub const GET_MULTIPLE_ACCOUNTS_CYCLES_PER_ADDRESS: u128 = 100_000_000;

/// The bytes of a `getMultipleAccounts` JSON-RPC response outside the account
/// list: the JSON-RPC envelope, the response context and the allowance for the
/// HTTP headers that the SOL RPC canister includes in its own estimates, since
/// it uses an estimate passed with a request as the response size limit as is.
pub const GET_MULTIPLE_ACCOUNTS_RESPONSE_BASE_BYTES: u64 = 128 + SOL_RPC_RESPONSE_HEADER_BYTES;

/// The bytes one account without data adds to a `getMultipleAccounts` JSON-RPC
/// response, at the largest lamport amount and rent epoch. The response size
/// estimate passed to the SOL RPC canister is sized by the number of addresses
/// so that the canister does not retry the call with doubled estimates, and
/// stays below its own estimate, which allows 512 bytes per account for the
/// account data the scan does not ask for.
pub const GET_MULTIPLE_ACCOUNTS_RESPONSE_BYTES_PER_ADDRESS: u64 = 160;

/// The allowance for the HTTP headers of a response that the SOL RPC canister
/// includes in the response size estimates of its typed methods.
const SOL_RPC_RESPONSE_HEADER_BYTES: u64 = 2 * 1024;

/// Cycles to attach for `getSignatureStatuses` RPC calls.
///
/// The SOL RPC canister charges about 2.3B cycles for a `getSignatureStatuses`
/// request for one signature and about 10.7B cycles for 256 signatures, the
/// largest batch, with the default 3-out-of-4 provider consensus and its
/// default response size estimate of 256 bytes per signature. The attached
/// amount leaves a wide margin for provider or price changes and for the SOL
/// RPC canister doubling the response size estimate after an oversized
/// response; the unused part is refunded. Up to `MAX_CONCURRENT_RPC_CALLS`
/// requests run at once, so the minter needs that many times this amount
/// available.
pub const GET_SIGNATURE_STATUSES_CYCLES: u128 = 50_000_000_000;

/// Cycles to attach for `getAccountInfo` RPC calls.
///
/// The SOL RPC canister charges about 2.1B cycles for a `getAccountInfo`
/// request with the default 3-out-of-4 provider consensus, comparable to
/// `getBalance` since a nonce account holds only 80 bytes of state. The
/// attached amount leaves a wide margin for provider or price changes; the
/// unused part is refunded.
pub const GET_ACCOUNT_INFO_CYCLES: u128 = 10_000_000_000;

/// Cost in lamports per signature included in a Solana transaction.
///
/// See <https://solana.com/docs/core/fees#base-fee>.
pub const FEE_PER_SIGNATURE: u64 = 5_000;

/// Minimum lamport balance required to keep a zero-data Solana account
/// rent-exempt (i.e. exempt from being purged by the runtime).
///
/// See <https://solana.com/docs/core/rent#rent-exempt-minimum>.
pub const RENT_EXEMPTION_THRESHOLD: u64 = 890_880;
