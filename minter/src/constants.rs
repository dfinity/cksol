use std::time::Duration;

/// Maximum number of concurrent calls to the SOL RPC canister.
pub const MAX_CONCURRENT_RPC_CALLS: usize = 10;

/// Short cooldown before rescheduling a timer that has more work to do.
pub const RESCHEDULE_DELAY: Duration = Duration::from_secs(1);

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
