use std::{num::NonZeroUsize, time::Duration};

/// Maximum number of concurrent calls to the SOL RPC canister.
pub const MAX_CONCURRENT_RPC_CALLS: usize = 10;

/// Maximum number of attempts to fetch a recent block, each attempt consisting of
/// at most one `getSlot` and one `getBlock` call.
pub const GET_RECENT_BLOCK_MAX_TRIES: NonZeroUsize =
    NonZeroUsize::new(3).expect("BUG: the maximum number of tries must be non-zero");

/// Interval of the timer sweeping queued deposits, the same as withdrawal processing.
pub const SWEEP_DEPOSITS_DELAY: Duration = Duration::from_mins(1);

/// Matches the ICP HTTPS outcall response limit for variable-length RPC calls
/// such as `getTransaction` and `getSignatureStatuses`:
/// https://docs.internetcomputer.org/references/ic-interface-spec#ic-http_request
pub const MAX_HTTP_OUTCALL_RESPONSE_BYTES: u64 = 2_000_000;

/// Cycles to attach for `getTransaction` RPC calls.
pub const GET_TRANSACTION_CYCLES: u128 = 50_000_000_000;

/// Cycles to attach for `getBalance` RPC calls.
///
/// The SOL RPC canister charges about 2.1B cycles for a `getBalance` request
/// with the default 3-out-of-4 provider consensus, comparable to `getSlot`
/// given the similarly small response (see section 3.3.2 of the design). The
/// attached amount leaves a wide margin for provider or price changes; the
/// unused part is refunded and never charged to the caller of `deposit_sol`.
pub const GET_BALANCE_CYCLES: u128 = 10_000_000_000;

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
