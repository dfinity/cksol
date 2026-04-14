/// Maximum number of concurrent calls to the SOL RPC canister.
pub const MAX_CONCURRENT_RPC_CALLS: usize = 10;

/// Maximum number of rounds per timer invocation.
/// Each round issues up to [`MAX_CONCURRENT_RPC_CALLS`] parallel RPC calls.
pub const MAX_TIMER_ROUNDS: usize = 5;

/// Matches the ICP HTTPS outcall response limit for variable-length RPC calls
/// such as `getTransaction` and `getSignatureStatuses`:
/// https://docs.internetcomputer.org/references/ic-interface-spec#ic-http_request
pub const MAX_HTTP_OUTCALL_RESPONSE_BYTES: u64 = 2_000_000;

/// Cycles to attach for `getSignatureStatuses` RPC calls.
pub const GET_SIGNATURE_STATUSES_CYCLES: u128 = 1_000_000_000_000;

/// Cost in lamports per signature included in a Solana transaction.
///
/// See <https://solana.com/docs/core/fees#base-fee>.
pub const FEE_PER_SIGNATURE: u64 = 5_000;
