//! Types used for lifecycle management of the ckSOL minter canister.
//!
//! Types in this module are unstable and breaking changes do not break the canister API.

#![forbid(unsafe_code)]

#[cfg(feature = "event")]
pub mod event;
#[cfg(feature = "log")]
pub mod log;

use candid::{CandidType, Principal};
use serde::{Deserialize, Serialize};
use sol_rpc_types::{Lamport, SolanaCluster};
use std::fmt;

/// The ckSOL minter service arguments.
#[derive(Clone, Debug, CandidType, Deserialize)]
#[cfg_attr(feature = "event", derive(minicbor::Encode, minicbor::Decode))]
pub enum MinterArg {
    /// Initialization arguments.
    #[cfg_attr(feature = "event", n(0))]
    Init(#[cfg_attr(feature = "event", n(0))] InitArgs),
    /// Upgrade arguments.
    #[cfg_attr(feature = "event", n(1))]
    Upgrade(#[cfg_attr(feature = "event", n(0))] UpgradeArgs),
}

/// The installation args for the ckSOL minter canister.
#[derive(Clone, Eq, PartialEq, Debug, CandidType, Deserialize)]
#[cfg_attr(feature = "event", derive(minicbor::Encode, minicbor::Decode))]
pub struct InitArgs {
    /// The canister ID of the SOL RPC canister.
    #[cfg_attr(feature = "event", n(0), cbor(with = "icrc_cbor::principal"))]
    pub sol_rpc_canister_id: Principal,
    /// The canister ID of the ckSOL ledger canister.
    #[cfg_attr(feature = "event", n(1), cbor(with = "icrc_cbor::principal"))]
    pub ledger_canister_id: Principal,
    /// The master Ed25519 key name.
    #[cfg_attr(feature = "event", n(2))]
    pub master_key_name: Ed25519KeyName,
    /// Minimum withdrawal amount in lamports.
    #[cfg_attr(feature = "event", n(3))]
    pub minimum_withdrawal_amount: Lamport,
    /// Minimum deposit amount in lamports.
    #[cfg_attr(feature = "event", n(4))]
    pub minimum_deposit_amount: Lamport,
    /// The withdrawal fee in lamports.
    #[cfg_attr(feature = "event", n(5))]
    pub withdrawal_fee: Lamport,
    /// Minimum cycles the caller must attach when calling `deposit_sol`.
    #[cfg_attr(feature = "event", n(6))]
    pub deposit_sol_required_cycles: u64,
    /// The Solana network to use.
    #[cfg_attr(feature = "event", n(7))]
    pub solana_network: SolanaNetwork,
    /// Extra cycles charged per `deposit_sol` call to offset the cost of the sweep.
    #[cfg_attr(feature = "event", n(8))]
    pub deposit_sol_fee: u64,
    /// The base58 addresses of the durable nonce accounts reserved for withdrawal transactions.
    #[cfg_attr(feature = "event", n(9))]
    pub nonce_accounts: Vec<String>,
    /// The fee in lamports deducted from the minted amount of a deposit detected by the
    /// automatic flow. The automatic flow stays unavailable until it is set.
    #[cfg_attr(feature = "event", n(10))]
    pub automatic_deposit_fee: Option<Lamport>,
}

/// The upgrade args for the ckSOL minter canister.
#[derive(Clone, Default, Eq, PartialEq, Debug, CandidType, Deserialize)]
#[cfg_attr(feature = "event", derive(minicbor::Encode, minicbor::Decode))]
pub struct UpgradeArgs {
    /// The canister ID of the SOL RPC canister.
    #[cfg_attr(feature = "event", n(0), cbor(with = "icrc_cbor::principal::option"))]
    pub sol_rpc_canister_id: Option<Principal>,
    /// The new minimum withdrawal amount in lamports.
    #[cfg_attr(feature = "event", n(1))]
    pub minimum_withdrawal_amount: Option<Lamport>,
    /// The new minimum deposit amount in lamports.
    #[cfg_attr(feature = "event", n(2))]
    pub minimum_deposit_amount: Option<Lamport>,
    /// The new withdrawal fee in lamports.
    #[cfg_attr(feature = "event", n(3))]
    pub withdrawal_fee: Option<Lamport>,
    /// New minimum cycles the caller must attach when calling `deposit_sol`.
    #[cfg_attr(feature = "event", n(4))]
    pub deposit_sol_required_cycles: Option<u64>,
    /// New extra cycles charged per `deposit_sol` call to offset the cost of the sweep.
    #[cfg_attr(feature = "event", n(5))]
    pub deposit_sol_fee: Option<u64>,
    /// The base58 addresses of durable nonce accounts to add to the withdrawal pool.
    #[cfg_attr(feature = "event", n(6))]
    pub nonce_accounts_to_add: Option<Vec<String>>,
    /// The new fee in lamports deducted from the minted amount of a deposit detected by the
    /// automatic flow.
    #[cfg_attr(feature = "event", n(7))]
    pub automatic_deposit_fee: Option<Lamport>,
    /// Registers classic SPL tokens. Existing registrations cannot be replaced.
    #[cfg_attr(feature = "event", n(8))]
    pub add_spl_tokens: Option<Vec<SplTokenRegistration>>,
    /// Updates deposit settings of registered SPL tokens.
    #[cfg_attr(feature = "event", n(9))]
    pub update_spl_tokens: Option<Vec<SplTokenUpdate>>,
    /// Automatic operating-SOL refill settings. Omitted values retain the configuration.
    #[cfg_attr(feature = "event", n(10))]
    pub operating_balance_refill: Option<OperatingRefillConfig>,
    /// New cycle fee charged for a manually accepted SPL deposit, excluding its RPC cost.
    #[cfg_attr(feature = "event", n(11))]
    pub spl_deposit_consolidation_fee: Option<u64>,
    /// New fee in lamports of ckSOL charged for an SPL withdrawal, excluding the rent reserve
    /// of the destination token account.
    #[cfg_attr(feature = "event", n(12))]
    pub spl_withdrawal_fee: Option<Lamport>,
    /// Halts or resumes SPL withdrawals. The minter halts them by itself when a withdrawal
    /// pays more rent than the reserve, and only `Some(false)` resumes them.
    #[cfg_attr(feature = "event", n(13))]
    pub spl_withdrawals_halted: Option<bool>,
}

/// The Solana network to connect to via the SOL RPC canister.
#[derive(Clone, Copy, Eq, PartialEq, Debug, Default, CandidType, Deserialize, Serialize)]
#[cfg_attr(feature = "event", derive(minicbor::Encode, minicbor::Decode))]
pub enum SolanaNetwork {
    /// Mainnet: live production environment.
    #[default]
    #[cfg_attr(feature = "event", n(0))]
    Mainnet,
    /// Devnet: testing with public accessibility.
    #[cfg_attr(feature = "event", n(1))]
    Devnet,
    /// Testnet: stress-testing for network upgrades.
    #[cfg_attr(feature = "event", n(2))]
    Testnet,
}

impl From<SolanaNetwork> for SolanaCluster {
    fn from(network: SolanaNetwork) -> Self {
        match network {
            SolanaNetwork::Mainnet => SolanaCluster::Mainnet,
            SolanaNetwork::Devnet => SolanaCluster::Devnet,
            SolanaNetwork::Testnet => SolanaCluster::Testnet,
        }
    }
}

/// The ID of one of the ICP root keys.
/// See the [tEdDSA documentation](https://internetcomputer.org/docs/building-apps/network-features/signatures/t-schnorr#signing-messages-and-transactions)
/// for more details.
#[derive(Clone, Copy, Eq, PartialEq, Debug, Default, CandidType, Deserialize, Serialize)]
#[cfg_attr(feature = "event", derive(minicbor::Encode, minicbor::Decode))]
pub enum Ed25519KeyName {
    /// Only available on the local development environment started by `dfx`.
    #[cfg_attr(feature = "event", n(0))]
    LocalDevelopment,
    /// Test key available on the ICP mainnet.
    #[cfg_attr(feature = "event", n(1))]
    MainnetTestKey1,
    /// Production key available on the ICP mainnet.
    #[default]
    #[cfg_attr(feature = "event", n(2))]
    MainnetProdKey1,
}

impl fmt::Display for Ed25519KeyName {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::LocalDevelopment => write!(f, "dfx_test_key"),
            Self::MainnetTestKey1 => write!(f, "test_key_1"),
            Self::MainnetProdKey1 => write!(f, "key_1"),
        }
    }
}

/// Proposal-supplied configuration of a classic SPL token.
/// Mint and ledger properties are verified outside the minter before registration.
#[derive(Clone, Eq, PartialEq, Debug, CandidType, Deserialize)]
#[cfg_attr(feature = "event", derive(minicbor::Encode, minicbor::Decode))]
pub struct SplTokenRegistration {
    /// Base58 mint address.
    #[cfg_attr(feature = "event", n(0))]
    pub mint: String,
    /// Base58 classic SPL Token program address.
    #[cfg_attr(feature = "event", n(1))]
    pub token_program: String,
    /// Decimals shared by the mint and ckSPL ledger.
    #[cfg_attr(feature = "event", n(2))]
    pub decimals: u8,
    /// The token's ckSPL ledger canister.
    #[cfg_attr(feature = "event", n(3), cbor(with = "icrc_cbor::principal"))]
    pub ledger_id: Principal,
    /// Optional index canister for client discovery.
    #[cfg_attr(feature = "event", n(4), cbor(with = "icrc_cbor::principal::option"))]
    pub index_id: Option<Principal>,
    /// The ckSPL ledger symbol.
    #[cfg_attr(feature = "event", n(5))]
    pub symbol: String,
    /// Minimum net deposit in the token's smallest units.
    #[cfg_attr(feature = "event", n(6))]
    pub minimum_deposit_amount: u64,
    /// Fixed deposit fee in the token's smallest units.
    #[cfg_attr(feature = "event", n(7))]
    pub deposit_fee: u64,
    /// Whether new deposits are paused.
    #[cfg_attr(feature = "event", n(8))]
    pub deposits_paused: bool,
    /// Withdrawal fee in the token's smallest units, deducted from the withdrawn amount.
    /// Omitted means no fee.
    #[cfg_attr(feature = "event", n(9))]
    pub withdrawal_fee_token: Option<u64>,
}

/// Changes to token settings. Omitted fields retain their existing values.
#[derive(Clone, Eq, PartialEq, Debug, CandidType, Deserialize)]
#[cfg_attr(feature = "event", derive(minicbor::Encode, minicbor::Decode))]
pub struct SplTokenUpdate {
    /// Base58 mint address of an already registered token.
    #[cfg_attr(feature = "event", n(0))]
    pub mint: String,
    /// New minimum net deposit in the token's smallest units.
    #[cfg_attr(feature = "event", n(1))]
    pub minimum_deposit_amount: Option<u64>,
    /// New deposit fee; zero disables the fee.
    #[cfg_attr(feature = "event", n(2))]
    pub deposit_fee: Option<u64>,
    /// Whether new deposits are paused.
    #[cfg_attr(feature = "event", n(3))]
    pub deposits_paused: Option<bool>,
    /// New withdrawal fee; zero disables the fee.
    #[cfg_attr(feature = "event", n(4))]
    pub withdrawal_fee_token: Option<u64>,
}

/// Automatic conversion of donated ckSOL into unbacked operating SOL.
#[derive(Clone, Copy, Eq, PartialEq, Debug, CandidType, Deserialize)]
#[cfg_attr(feature = "event", derive(minicbor::Encode, minicbor::Decode))]
pub struct OperatingRefillConfig {
    /// Stops new refills when false; an unresolved burn is still tracked.
    #[cfg_attr(feature = "event", n(0))]
    pub enabled: bool,
    /// Refill when the operating balance is strictly below this many lamports.
    #[cfg_attr(feature = "event", n(1))]
    pub low_water_mark: u64,
    /// Desired operating balance in lamports after a refill.
    #[cfg_attr(feature = "event", n(2))]
    pub target: u64,
}

// Initial defaults are part of event replay. Change live settings through UpgradeArgs,
// rather than changing these values when tuning refills in a later release.
impl Default for OperatingRefillConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            low_water_mark: 10_000_000, // 0.01 SOL
            target: 100_000_000,        // 0.1 SOL
        }
    }
}
