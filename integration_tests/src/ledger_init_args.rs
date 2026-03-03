//! Since the init args for the ICRC1 ledger are not published in a public crate,
//! redefine them here to initialize the canister correctly.
use candid::{CandidType, Deserialize, Nat, Principal};
use cksol_types::MAX_SERIALIZED_MEMO_BYTES;
use icrc_ledger_types::{icrc::generic_value::Value, icrc1::account::Account};
use serde::Serialize;

pub const LEDGER_TRANSFER_FEE: u64 = 50;
const NNS_ROOT_PRINCIPAL: Principal = Principal::from_slice(&[0_u8, 0, 0, 0, 0, 0, 0, 3, 1, 1]);
const FEE_COLLECTOR_SUBACCOUNT: [u8; 32] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x0f,
    0xee,
];
const CKSOL_LOGO: &str = "data:image/svg+xml;base64,PHN2ZyB3aWR0aD0iMzYwIiBoZWlnaHQ9IjM2MCIgdmlld0JveD0iMCAwIDM2MCAzNjAiIGZpbGw9Im5vbmUiIHhtbG5zPSJodHRwOi8vd3d3LnczLm9yZy8yMDAwL3N2ZyI+CjxnIGNsaXAtcGF0aD0idXJsKCNjbGlwMF8xMDIzXzQ5KSI+CjxwYXRoIGQ9Ik0xODAgMEMyNzkuNCAwIDM2MCA4MC42IDM2MCAxODBDMzYwIDI3OS40IDI3OS40IDM2MCAxODAgMzYwQzgwLjYgMzYwIDAgMjc5LjQgMCAxODBDMCA4MC42IDgwLjYgMCAxODAgMFoiIGZpbGw9IiMzQjAwQjkiLz4KPHBhdGggZmlsbC1ydWxlPSJldmVub2RkIiBjbGlwLXJ1bGU9ImV2ZW5vZGQiIGQ9Ik00MC4zOTk4IDE5MC40QzQ1LjM5OTggMjU5LjQgMTAwLjYgMzE0LjYgMTY5LjYgMzE5LjZWMzM1LjJDOTEuOTk5OCAzMzAgMjkuOTk5OCAyNjggMjQuNzk5OCAxOTAuNEg0MC4zOTk4WiIgZmlsbD0idXJsKCNwYWludDBfbGluZWFyXzEwMjNfNDkpIi8+CjxwYXRoIGZpbGwtcnVsZT0iZXZlbm9kZCIgY2xpcC1ydWxlPSJldmVub2RkIiBkPSJNMTY5LjYgNDAuNEMxMDAuNiA0NS40IDQ1LjM5OTggMTAwLjYgNDAuMzk5OCAxNjkuNkgyNC43OTk4QzI5Ljc5OTggOTIgOTEuOTk5OCAyOS44IDE2OS42IDI0LjhWNDAuNFoiIGZpbGw9IiMyOUFCRTIiLz4KPHBhdGggZmlsbC1ydWxlPSJldmVub2RkIiBjbGlwLXJ1bGU9ImV2ZW5vZGQiIGQ9Ik0zMTkuNiAxNjkuNEMzMTQuNiAxMDAuNCAyNTkuNCA0NS4yIDE5MC40IDQwLjJWMjQuNkMyNjggMjkuOCAzMzAuMiA5MS44IDMzNS4yIDE2OS40SDMxOS42WiIgZmlsbD0idXJsKCNwYWludDFfbGluZWFyXzEwMjNfNDkpIi8+CjxwYXRoIGZpbGwtcnVsZT0iZXZlbm9kZCIgY2xpcC1ydWxlPSJldmVub2RkIiBkPSJNMTkwLjQgMzE5LjZDMjU5LjQgMzE0LjYgMzE0LjYgMjU5LjQgMzE5LjYgMTkwLjRIMzM1LjJDMzMwLjIgMjY4IDI2OCAzMzAgMTkwLjQgMzM1LjJWMzE5LjZaIiBmaWxsPSIjMjlBQkUyIi8+CjxnIGNsaXAtcGF0aD0idXJsKCNjbGlwMV8xMDIzXzQ5KSI+CjxwYXRoIGQ9Ik0yNjQuMTI1IDIyMi43ODFMMjM2LjA2MSAyNTIuMTAyQzIzNS40NTEgMjUyLjczOCAyMzQuNzEzIDI1My4yNDYgMjMzLjg5MyAyNTMuNTkzQzIzMy4wNzIgMjUzLjk0IDIzMi4xODcgMjU0LjExOSAyMzEuMjkzIDI1NC4xMTlIOTguMjU4Qzk3LjYyMzIgMjU0LjExOSA5Ny4wMDIyIDI1My45MzggOTYuNDcxNCAyNTMuNTk5Qzk1Ljk0MDYgMjUzLjI2IDk1LjUyMyAyNTIuNzc3IDk1LjI3IDI1Mi4yMUM5NS4wMTcgMjUxLjY0MyA5NC45Mzk1IDI1MS4wMTYgOTUuMDQ3MiAyNTAuNDA3Qzk1LjE1NDggMjQ5Ljc5NyA5NS40NDI5IDI0OS4yMzIgOTUuODc2IDI0OC43NzlMMTIzLjk2MSAyMTkuNDU5QzEyNC41NjkgMjE4LjgyNCAxMjUuMzA1IDIxOC4zMTcgMTI2LjEyMiAyMTcuOTdDMTI2Ljk0IDIxNy42MjMgMTI3LjgyMiAyMTcuNDQzIDEyOC43MTQgMjE3LjQ0MkgyNjEuNzQyQzI2Mi4zNzcgMjE3LjQ0MiAyNjIuOTk4IDIxNy42MjMgMjYzLjUyOSAyMTcuOTYyQzI2NC4wNTkgMjE4LjMwMSAyNjQuNDc3IDIxOC43ODQgMjY0LjczMSAyMTkuMzUxQzI2NC45ODMgMjE5LjkxOCAyNjUuMDYxIDIyMC41NDQgMjY0Ljk1MyAyMjEuMTU0QzI2NC44NDUgMjIxLjc2MyAyNjQuNTU3IDIyMi4zMjkgMjY0LjEyNSAyMjIuNzgxWk0yMzYuMDYxIDE2My43MzhDMjM1LjQ1MSAxNjMuMTAxIDIzNC43MTMgMTYyLjU5MyAyMzMuODkzIDE2Mi4yNDZDMjMzLjA3MiAxNjEuODk5IDIzMi4xODcgMTYxLjcyIDIzMS4yOTMgMTYxLjcyMUg5OC4yNThDOTcuNjIzMiAxNjEuNzIxIDk3LjAwMjIgMTYxLjkwMiA5Ni40NzE0IDE2Mi4yNDFDOTUuOTQwNiAxNjIuNTggOTUuNTIzIDE2My4wNjMgOTUuMjcgMTYzLjYzQzk1LjAxNyAxNjQuMTk3IDk0LjkzOTUgMTY0LjgyNCA5NS4wNDcyIDE2NS40MzNDOTUuMTU0OCAxNjYuMDQyIDk1LjQ0MjkgMTY2LjYwOCA5NS44NzYgMTY3LjA2TDEyMy45NjEgMTk2LjM4MUMxMjQuNTY5IDE5Ny4wMTYgMTI1LjMwNSAxOTcuNTIzIDEyNi4xMjIgMTk3Ljg3QzEyNi45NCAxOTguMjE3IDEyNy44MjIgMTk4LjM5NyAxMjguNzE0IDE5OC4zOThIMjYxLjc0MkMyNjIuMzc3IDE5OC4zOTggMjYyLjk5OCAxOTguMjE3IDI2My41MjkgMTk3Ljg3OEMyNjQuMDU5IDE5Ny41MzkgMjY0LjQ3NyAxOTcuMDU2IDI2NC43MzEgMTk2LjQ4OUMyNjQuOTgzIDE5NS45MjIgMjY1LjA2MSAxOTUuMjk1IDI2NC45NTMgMTk0LjY4NkMyNjQuODQ1IDE5NC4wNzYgMjY0LjU1NyAxOTMuNTExIDI2NC4xMjUgMTkzLjA1OUwyMzYuMDYxIDE2My43MzhaTTk4LjI1OCAxNDIuNjc3SDIzMS4yOTNDMjMyLjE4NyAxNDIuNjc3IDIzMy4wNzIgMTQyLjQ5OSAyMzMuODkzIDE0Mi4xNTJDMjM0LjcxMyAxNDEuODA1IDIzNS40NTEgMTQxLjI5NyAyMzYuMDYxIDE0MC42NkwyNjQuMTI1IDExMS4zMzlDMjY0LjU1NyAxMTAuODg3IDI2NC44NDUgMTEwLjMyMiAyNjQuOTUzIDEwOS43MTJDMjY1LjA2MSAxMDkuMTAzIDI2NC45ODMgMTA4LjQ3NiAyNjQuNzMxIDEwNy45MDlDMjY0LjQ3NyAxMDcuMzQyIDI2NC4wNTkgMTA2Ljg1OSAyNjMuNTI5IDEwNi41MkMyNjIuOTk4IDEwNi4xODEgMjYyLjM3NyAxMDYgMjYxLjc0MiAxMDZIMTI4LjcxNEMxMjcuODIyIDEwNi4wMDEgMTI2Ljk0IDEwNi4xODEgMTI2LjEyMiAxMDYuNTI4QzEyNS4zMDUgMTA2Ljg3NSAxMjQuNTY5IDEwNy4zODIgMTIzLjk2MSAxMDguMDE3TDk1Ljg4MzIgMTM3LjMzOEM5NS40NTA2IDEzNy43ODkgOTUuMTYyNiAxMzguMzU0IDk1LjA1NDcgMTM4Ljk2M0M5NC45NDY4IDEzOS41NzIgOTUuMDIzNyAxNDAuMTk4IDk1LjI3NTggMTQwLjc2NUM5NS41Mjc5IDE0MS4zMzIgOTUuOTQ0NCAxNDEuODE1IDk2LjQ3NDEgMTQyLjE1NEM5Ny4wMDM5IDE0Mi40OTQgOTcuNjIzOCAxNDIuNjc2IDk4LjI1OCAxNDIuNjc3WiIgZmlsbD0id2hpdGUiLz4KPC9nPgo8L2c+CjxkZWZzPgo8bGluZWFyR3JhZGllbnQgaWQ9InBhaW50MF9saW5lYXJfMTAyM180OSIgeDE9IjEzMC43MiIgeTE9IjMwNC4xMiIgeDI9IjMzLjQ3OTgiIHkyPSIyMjIuMjIiIGdyYWRpZW50VW5pdHM9InVzZXJTcGFjZU9uVXNlIj4KPHN0b3Agb2Zmc2V0PSIwLjIxIiBzdG9wLWNvbG9yPSIjRUQxRTc5Ii8+CjxzdG9wIG9mZnNldD0iMSIgc3RvcC1jb2xvcj0iIzUyMjc4NSIvPgo8L2xpbmVhckdyYWRpZW50Pgo8bGluZWFyR3JhZGllbnQgaWQ9InBhaW50MV9saW5lYXJfMTAyM180OSIgeDE9IjMwOS4zMiIgeTE9IjEyMy4wNiIgeDI9IjIxMi4wOCIgeTI9IjQxLjE2IiBncmFkaWVudFVuaXRzPSJ1c2VyU3BhY2VPblVzZSI+CjxzdG9wIG9mZnNldD0iMC4yMSIgc3RvcC1jb2xvcj0iI0YxNUEyNCIvPgo8c3RvcCBvZmZzZXQ9IjAuNjgiIHN0b3AtY29sb3I9IiNGQkIwM0IiLz4KPC9saW5lYXJHcmFkaWVudD4KPGNsaXBQYXRoIGlkPSJjbGlwMF8xMDIzXzQ5Ij4KPHJlY3Qgd2lkdGg9IjM2MCIgaGVpZ2h0PSIzNjAiIGZpbGw9IndoaXRlIi8+CjwvY2xpcFBhdGg+CjxjbGlwUGF0aCBpZD0iY2xpcDFfMTAyM180OSI+CjxyZWN0IHdpZHRoPSIxNzAiIGhlaWdodD0iMTQ4LjExOSIgZmlsbD0id2hpdGUiIHRyYW5zZm9ybT0idHJhbnNsYXRlKDk1IDEwNikiLz4KPC9jbGlwUGF0aD4KPC9kZWZzPgo8L3N2Zz4K";

pub fn ledger_init_args(
    minter_canister_id: Principal,
    initial_balances: Vec<(Account, Nat)>,
) -> LedgerArgument {
    LedgerArgument::Init(InitArgs {
        minting_account: Account::from(minter_canister_id),
        fee_collector_account: Some(Account {
            owner: minter_canister_id,
            subaccount: Some(FEE_COLLECTOR_SUBACCOUNT),
        }),
        initial_balances,
        transfer_fee: Nat::from(LEDGER_TRANSFER_FEE),
        decimals: Some(9),
        token_name: "ckSOL".to_string(),
        token_symbol: "ckSOL".to_string(),
        metadata: vec![(
            "icrc1:logo".to_string(),
            Value::Text(CKSOL_LOGO.to_string()),
        )],
        archive_options: ArchiveOptions {
            trigger_threshold: 2_000,
            num_blocks_to_archive: 1_0000,
            node_max_memory_size_bytes: Some(3_221_225_472),
            max_message_size_bytes: None,
            controller_id: NNS_ROOT_PRINCIPAL,
            more_controller_ids: None,
            cycles_for_archive_creation: Some(100_000_000_000_000),
            max_transactions_per_response: None,
        },
        max_memo_length: Some(MAX_SERIALIZED_MEMO_BYTES),
        feature_flags: None,
        index_principal: None,
    })
}

#[derive(Clone, Eq, PartialEq, Debug, CandidType, Deserialize)]
pub enum LedgerArgument {
    Init(InitArgs),
}

#[derive(Clone, Eq, PartialEq, Debug, CandidType, Deserialize)]
pub struct InitArgs {
    pub minting_account: Account,
    pub fee_collector_account: Option<Account>,
    pub initial_balances: Vec<(Account, Nat)>,
    pub transfer_fee: Nat,
    pub decimals: Option<u8>,
    pub token_name: String,
    pub token_symbol: String,
    pub metadata: Vec<(String, Value)>,
    pub archive_options: ArchiveOptions,
    pub max_memo_length: Option<u16>,
    pub feature_flags: Option<FeatureFlags>,
    pub index_principal: Option<Principal>,
}

#[derive(Clone, Eq, PartialEq, Debug, CandidType, Deserialize, Serialize)]
pub struct FeatureFlags {
    pub icrc2: bool,
}

impl FeatureFlags {
    const fn const_default() -> Self {
        Self { icrc2: true }
    }
}

impl Default for FeatureFlags {
    fn default() -> Self {
        Self::const_default()
    }
}

#[derive(Clone, Eq, PartialEq, Debug, CandidType, Deserialize, Serialize)]
pub struct ArchiveOptions {
    /// The number of blocks which, when exceeded, will trigger an archiving
    /// operation.
    pub trigger_threshold: usize,
    /// The number of blocks to archive when trigger threshold is exceeded.
    pub num_blocks_to_archive: usize,
    pub node_max_memory_size_bytes: Option<u64>,
    pub max_message_size_bytes: Option<u64>,
    pub controller_id: Principal,
    // More principals to add as controller of the archive.
    #[serde(default)]
    pub more_controller_ids: Option<Vec<Principal>>,
    // cycles to use for the call to create a new archive canister.
    #[serde(default)]
    pub cycles_for_archive_creation: Option<u64>,
    // Max transactions returned by the [get_transactions] endpoint.
    #[serde(default)]
    pub max_transactions_per_response: Option<u64>,
}
