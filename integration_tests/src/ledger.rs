//! Since the init args for the ICRC1 ledger are not published in a public crate,
//! redefine them here to initialize the canister correctly.
use candid::{CandidType, Deserialize, Nat, Principal};
use cksol_types::MAX_SERIALIZED_MEMO_BYTES;
use icrc_ledger_types::{icrc::generic_value::Value, icrc1::account::Account};
use serde::Serialize;

const LEDGER_TRANSFER_FEE: u64 = 50;
const NNS_ROOT_PRINCIPAL: Principal = Principal::from_slice(&[0_u8]);
const FEE_COLLECTOR_SUBACCOUNT: [u8; 32] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x0f,
    0xee,
];

pub fn ledger_init_args(minter_canister_id: Principal) -> LedgerArgument {
    LedgerArgument::Init(InitArgs {
        minting_account: Account::from(minter_canister_id),
        fee_collector_account: Some(Account {
            owner: minter_canister_id,
            subaccount: Some(FEE_COLLECTOR_SUBACCOUNT),
        }),
        initial_balances: vec![],
        transfer_fee: Nat::from(LEDGER_TRANSFER_FEE),
        decimals: Some(9),
        token_name: "ckSOL".to_string(),
        token_symbol: "ckSOL".to_string(),
        metadata: vec![],
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
