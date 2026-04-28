/// Maximum number of work items the executor processes in a single batch.
pub const MAX_CONCURRENT_RPC_CALLS: usize = 10;

/// Maximum number of concurrent SOL RPC calls from user-facing endpoints
/// (e.g. `process_deposit`) that run outside of the timer-driven executor.
pub const MAX_CONCURRENT_USER_RPC_CALLS: u32 = 10;

/// Maximum number of `getSignaturesForAddress` results to request per polled account.
pub const MAX_TRANSACTIONS_PER_ACCOUNT: usize = 10;

/// Matches the ICP HTTPS outcall response limit for variable-length RPC calls
/// such as `getTransaction` and `getSignatureStatuses`:
/// https://docs.internetcomputer.org/references/ic-interface-spec#ic-http_request
pub const MAX_HTTP_OUTCALL_RESPONSE_BYTES: u64 = 2_000_000;

/// Cycles to attach for `getTransaction` RPC calls.
pub const GET_TRANSACTION_CYCLES: u128 = 50_000_000_000;

/// Cycles to attach for `getSignatureStatuses` RPC calls.
pub const GET_SIGNATURE_STATUSES_CYCLES: u128 = 1_000_000_000_000;

/// Cost in lamports per signature included in a Solana transaction.
///
/// See <https://solana.com/docs/core/fees#base-fee>.
pub const FEE_PER_SIGNATURE: u64 = 5_000;

/// Minimum lamport balance required to keep a zero-data Solana account
/// rent-exempt (i.e. exempt from being purged by the runtime).
///
/// See <https://solana.com/docs/core/rent#rent-exempt-minimum>.
pub const RENT_EXEMPTION_THRESHOLD: u64 = 890_880;
