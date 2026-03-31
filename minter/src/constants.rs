/// Maximum number of concurrent calls to the SOL RPC canister.
pub const MAX_CONCURRENT_RPC_CALLS: usize = 10;

/// Maximum expected response size for variable-length RPC calls (e.g., getTransaction,
/// getSignatureStatuses). Matches the ICP HTTPS outcall response limit:
/// https://docs.internetcomputer.org/references/ic-interface-spec#ic-http_request
pub const MAX_RESPONSE_BYTES: u64 = 2_000_000;
