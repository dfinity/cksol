use crate::{
    address::{MINTER_DERIVATION_PATH, account_address, derivation_path},
    constants::{FEE_PER_SIGNATURE, RENT_EXEMPTION_THRESHOLD},
    rpc::{BlockHeight, FetchedTransaction},
    sol_transfer::build_batch_withdrawal_message,
    state::{
        DepositBalance, QueuedDeposit, SchnorrPublicKey, State, Sweep,
        event::{Event, EventType, VersionedMessage},
        init_once_state, mutate_state,
    },
    storage::with_event_iter,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use candid::Principal;
use cksol_types::DepositSolId;
use cksol_types_internal::{Ed25519KeyName, InitArgs, SolanaNetwork};
use ic_cdk_management_canister::SchnorrPublicKeyResult;
use ic_ed25519::{PocketIcMasterPublicKeyId, PublicKey};
use icrc_ledger_types::icrc1::account::Account;
use sol_rpc_types::{Lamport, MultiRpcResult};
use solana_address::{Address, address};
use solana_nonce::{
    state::{Data as NonceData, DurableNonce, State as NonceState},
    versions::Versions as NonceVersions,
};
use solana_transaction_status_client_types::{
    EncodedConfirmedTransactionWithStatusMeta, EncodedTransaction,
    EncodedTransactionWithStatusMeta, TransactionBinaryEncoding, UiLoadedAddresses,
    UiTransactionStatusMeta, option_serializer::OptionSerializer,
};
use std::{collections::VecDeque, str::FromStr};

pub mod flow;
pub mod runtime;
pub mod signer;
mod stubs;
#[cfg(test)]
mod tests;

pub type GetTransactionResult =
    MultiRpcResult<Option<sol_rpc_types::EncodedConfirmedTransactionWithStatusMeta>>;

pub const BLOCK_INDEX: u64 = 98763_u64;
pub const DEPOSIT_SOL_FEE: u128 = 10_000_000_000; // 0.01T cycles
pub const WITHDRAWAL_FEE: Lamport = 1_000_000; // 0.001 SOL
pub const MINIMUM_WITHDRAWAL_AMOUNT: Lamport = 2_000_000; // 0.002 SOL
pub const MINTER_ACCOUNT: Account = Account {
    owner: runtime::TEST_CANISTER_ID,
    subaccount: None,
};
/// The minter's main Solana address under the test master key: the raw master public key.
pub const MINTER_ADDRESS: Address = address!("Fkt68XQXBDDBGBNNjFh8GM27ffpZGmncUdDG19njnRvY");
/// The durable nonce account in the pool configured by [`init_state`].
pub const NONCE_ACCOUNT: Address = address!("US517G5965aydkZ46HS38QLi7UQiSojurfbQfKCELFx");
pub const MINIMUM_DEPOSIT_AMOUNT: Lamport = 20_000_000; // 0.02 SOL
pub const DEPOSIT_SOL_REQUIRED_CYCLES: u128 = 1_000_000_000_000;

pub fn sol_rpc_canister_id() -> Principal {
    Principal::from_slice(&[1_u8; 20])
}

pub fn ledger_canister_id() -> Principal {
    Principal::from_slice(&[2_u8; 20])
}

pub fn valid_init_args() -> InitArgs {
    InitArgs {
        sol_rpc_canister_id: sol_rpc_canister_id(),
        ledger_canister_id: ledger_canister_id(),
        master_key_name: Ed25519KeyName::default(),
        minimum_withdrawal_amount: MINIMUM_WITHDRAWAL_AMOUNT,
        minimum_deposit_amount: MINIMUM_DEPOSIT_AMOUNT,
        withdrawal_fee: WITHDRAWAL_FEE,
        deposit_sol_required_cycles: DEPOSIT_SOL_REQUIRED_CYCLES as u64,
        solana_network: SolanaNetwork::Mainnet,
        deposit_sol_fee: DEPOSIT_SOL_FEE as u64,
        nonce_accounts: vec![],
    }
}

pub fn init_state() {
    init_state_with_args(init_args_with_nonce_account());
}

pub fn init_args_with_nonce_account() -> InitArgs {
    InitArgs {
        nonce_accounts: vec![NONCE_ACCOUNT.to_string()],
        ..valid_init_args()
    }
}

pub fn init_state_with_args(init_args: InitArgs) {
    init_once_state(State::try_from(init_args).expect("Invalid init args"));
}

pub fn init_balance() {
    init_balance_to(u64::MAX / 2);
}

/// Credits the minter balance with exactly `amount` by sweeping a deposit of a
/// dedicated account through the whole flow, so the account is released again.
///
/// Queueing the funding deposit needs the minter public key, so this records it.
pub fn init_balance_to(amount: Lamport) {
    init_schnorr_master_key();
    let funding_deposit_id = crate::state::read_state(|state| state.deposits().next_id());
    let sweep_signature = signature(0xFF00 + funding_deposit_id as usize);

    events::queue_deposit(funding_deposit_id, account(0xFD), amount);
    events::submit_sweep(sweep_signature, vec![funding_deposit_id]);
    events::succeed_transaction(sweep_signature);
    events::credit_sweep(sweep_signature, amount);
    events::mint_swept_deposit(funding_deposit_id, 0xFE00 + funding_deposit_id);
}

pub fn init_schnorr_master_key() {
    mutate_state(|s| s.cache_minter_public_key(schnorr_master_key()));
}

/// The master key [`init_schnorr_master_key`] caches, as the management canister returns it,
/// for a test that starts without it cached and lets the minter fetch it.
pub fn schnorr_master_key_response() -> SchnorrPublicKeyResult {
    let master_key = schnorr_master_key();
    SchnorrPublicKeyResult {
        public_key: master_key.public_key.serialize_raw().to_vec(),
        chain_code: master_key.chain_code.to_vec(),
    }
}

pub fn schnorr_master_key() -> SchnorrPublicKey {
    SchnorrPublicKey {
        public_key: PublicKey::pocketic_key(PocketIcMasterPublicKeyId::Key1),
        chain_code: [1; 32],
    }
}

/// The event recorded when the minter fetches the master key of [`init_schnorr_master_key`].
pub fn minter_public_key_fetched_event() -> EventType {
    let master_key = schnorr_master_key();
    EventType::MinterPublicKeyFetched {
        public_key: master_key.public_key,
        chain_code: master_key.chain_code,
    }
}

/// Returns a [`Signature`] unique for any `usize` index, derived from `i as u64` via le_bytes.
pub fn signature(i: usize) -> solana_signature::Signature {
    let mut bytes = [0u8; 64];
    bytes[..8].copy_from_slice(&(i as u64).to_le_bytes());
    solana_signature::Signature::from(bytes)
}

/// Returns an [`Address`] unique for any `usize` index, derived from `i as u64` via le_bytes.
pub fn address(i: usize) -> Address {
    let mut bytes = [0u8; 32];
    bytes[..8].copy_from_slice(&(i as u64).to_le_bytes());
    Address::from(bytes)
}

/// The block height used by fixtures whose test does not care about blockhash expiry.
pub const DEFAULT_BLOCK_HEIGHT: BlockHeight = BlockHeight::new(400_000_000);

/// Returns a [`ConfirmedBlock`] with a deterministic blockhash at
/// [`DEFAULT_BLOCK_HEIGHT`], for use in RPC mock stubs.
pub fn confirmed_block() -> sol_rpc_types::ConfirmedBlock {
    confirmed_block_at_height(DEFAULT_BLOCK_HEIGHT)
}

/// Returns a [`ConfirmedBlock`] with a deterministic blockhash at the given block height.
pub fn confirmed_block_at_height(block_height: BlockHeight) -> sol_rpc_types::ConfirmedBlock {
    sol_rpc_types::ConfirmedBlock {
        previous_blockhash: Default::default(),
        blockhash: solana_hash::Hash::from([0x42; 32]).into(),
        parent_slot: 0,
        block_time: None,
        block_height: Some(block_height.get()),
        signatures: None,
        rewards: None,
        num_reward_partitions: None,
        transactions: None,
    }
}

/// A successful transaction status at the `finalized` commitment level.
pub fn finalized_status() -> sol_rpc_types::TransactionStatus {
    sol_rpc_types::TransactionStatus {
        slot: 0,
        status: Ok(()),
        err: None,
        confirmation_status: Some(sol_rpc_types::TransactionConfirmationStatus::Finalized),
    }
}

/// A test durable nonce account address, distinct from any deposit address.
pub fn nonce_account_address() -> Address {
    Address::from([0x4E; 32])
}

/// Returns the batch withdrawal message of [`MINTER_ADDRESS`] that advances
/// `nonce_account`, carries `nonce_value` in place of a recent blockhash and
/// performs the given transfers.
pub fn withdrawal_batch_message(
    nonce_account: Address,
    nonce_value: solana_hash::Hash,
    transfers: &[(Address, Lamport)],
) -> solana_message::Message {
    build_batch_withdrawal_message(&MINTER_ADDRESS, &nonce_account, nonce_value, transfers)
        .expect("BUG: the withdrawal batch message exceeds the transaction size")
}

/// Returns the nonce value stored by [`nonce_account_info`] for the same `nonce_seed`.
pub fn durable_nonce(nonce_seed: usize) -> solana_hash::Hash {
    *DurableNonce::from_blockhash(&seed_hash(nonce_seed)).as_hash()
}

/// Returns a `getAccountInfo` response for an initialized durable nonce account with
/// the given authority, storing the nonce value [`durable_nonce`] of the same `nonce_seed`.
pub fn nonce_account_info(authority: Address, nonce_seed: usize) -> sol_rpc_types::AccountInfo {
    nonce_account_info_in_state(NonceState::Initialized(NonceData::new(
        authority,
        DurableNonce::from_blockhash(&seed_hash(nonce_seed)),
        FEE_PER_SIGNATURE,
    )))
}

/// Returns a `getAccountInfo` response for a nonce account that has not been initialized.
pub fn uninitialized_nonce_account_info() -> sol_rpc_types::AccountInfo {
    nonce_account_info_in_state(NonceState::Uninitialized)
}

/// Returns a `getAccountInfo` response for an initialized nonce account in the legacy format.
pub fn legacy_nonce_account_info(authority: Address) -> sol_rpc_types::AccountInfo {
    nonce_account_info_in_versions(NonceVersions::Legacy(Box::new(NonceState::Initialized(
        NonceData::new(
            authority,
            DurableNonce::from_blockhash(&seed_hash(1)),
            FEE_PER_SIGNATURE,
        ),
    ))))
}

fn nonce_account_info_in_state(state: NonceState) -> sol_rpc_types::AccountInfo {
    nonce_account_info_in_versions(NonceVersions::new(state))
}

fn nonce_account_info_in_versions(versions: NonceVersions) -> sol_rpc_types::AccountInfo {
    const SYSTEM_PROGRAM_ID: &str = "11111111111111111111111111111111";
    let data =
        bincode::serialize(&versions).expect("BUG: serializing a nonce account should succeed");
    sol_rpc_types::AccountInfo {
        lamports: 1_447_680,
        space: data.len() as u64,
        data: sol_rpc_types::AccountData::Binary(
            STANDARD.encode(data),
            sol_rpc_types::AccountEncoding::Base64,
        ),
        owner: SYSTEM_PROGRAM_ID.to_string(),
        executable: false,
        rent_epoch: u64::MAX,
    }
}

fn seed_hash(seed: usize) -> solana_hash::Hash {
    let mut bytes = [0u8; 32];
    bytes[..8].copy_from_slice(&(seed as u64).to_le_bytes());
    solana_hash::Hash::from(bytes)
}

/// Returns an [`Account`] with a deterministic principal derived from `i`.
/// The deposit of `account(deposit_id + 1)` with `1_000_000 * (deposit_id + 1)` sweepable
/// lamports, so that a sequence of deposits has distinct accounts and amounts, each covering
/// the fee of a full sweep.
pub fn queued_deposit(deposit_id: DepositSolId) -> QueuedDeposit {
    queued_deposit_of(
        account(deposit_id as usize + 1),
        1_000_000 * (deposit_id + 1),
    )
}

/// The deposit of the given account whose address holds the sweepable amount on top of the
/// rent exemption threshold.
pub fn queued_deposit_of(account: Account, sweepable_amount: Lamport) -> QueuedDeposit {
    QueuedDeposit {
        account,
        address: deposit_address(account),
        balance: DepositBalance::new(sweepable_amount + RENT_EXEMPTION_THRESHOLD)
            .expect("BUG: the balance covers the rent exemption threshold"),
    }
}

/// The sweep of the given deposits to [`MINTER_ADDRESS`].
pub fn planned_sweep(deposits: impl IntoIterator<Item = (DepositSolId, QueuedDeposit)>) -> Sweep {
    Sweep::plan(deposits, MINTER_ADDRESS)
}

/// The message submitted for the sweep of the given deposits to [`MINTER_ADDRESS`].
pub fn sweep_message(
    deposits: impl IntoIterator<Item = (DepositSolId, QueuedDeposit)>,
) -> VersionedMessage {
    planned_sweep(deposits)
        .sweep_message(solana_hash::Hash::default())
        .into()
}

/// The deposit address of the account under the master key of [`init_schnorr_master_key`].
pub fn deposit_address(account: Account) -> solana_address::Address {
    account_address(&schnorr_master_key(), &account)
}

/// The id of the `i`-th deposit a test queues, since ids are assigned in sequence.
pub fn deposit_id(i: usize) -> DepositSolId {
    i as DepositSolId
}

/// Returns an [`Account`] with a deterministic principal derived from `i`.
pub fn account(i: usize) -> Account {
    let mut bytes = [0u8; 29];
    bytes[..8].copy_from_slice(&(i as u64).to_le_bytes());
    Account {
        owner: Principal::from_slice(&bytes),
        subaccount: None,
    }
}

/// Returns the [`Signature`] that [`signer::MockSchnorrSigner`] produces the first time
/// `account` signs.
pub fn account_signature(account: &Account) -> solana_signature::Signature {
    account_signature_nth(account, 0)
}

/// Returns the [`Signature`] that [`signer::MockSchnorrSigner`] produces the
/// `occurrence`-th time `account` signs, counting from zero.
pub fn account_signature_nth(account: &Account, occurrence: usize) -> solana_signature::Signature {
    signer::derivation_path_signature(&derivation_path(account), occurrence)
}

/// Returns the [`Signature`] that [`signer::MockSchnorrSigner`] produces the first time
/// the minter's main address signs.
pub fn minter_signature() -> solana_signature::Signature {
    minter_signature_nth(0)
}

/// Returns the [`Signature`] that [`signer::MockSchnorrSigner`] produces the
/// `occurrence`-th time the minter's main address signs, counting from zero.
pub fn minter_signature_nth(occurrence: usize) -> solana_signature::Signature {
    signer::derivation_path_signature(&MINTER_DERIVATION_PATH, occurrence)
}

/// The [`FetchedTransaction`] that [`rpc::get_transaction`] returns for the given
/// `getTransaction` output.
pub fn fetched(outcome: EncodedConfirmedTransactionWithStatusMeta) -> FetchedTransaction {
    FetchedTransaction {
        transaction: outcome
            .transaction
            .transaction
            .decode()
            .expect("BUG: the fixture transaction should decode"),
        meta: outcome.transaction.meta,
    }
}

/// A sweep of four deposits that the minter submitted on devnet as transaction
/// `59vLxkN5YGgBrHGTQMCrntNi7CrAxfkQxYek2v3hUgKfujgGtfkSDZZmFyVw6S59uTH2FEwWcvntPiEkdN5Ep5W2`,
/// with the `getTransaction` response the minter settles it against, and ways to deviate
/// from that response.
pub mod devnet_sweep {
    use super::account;
    use crate::state::{DepositBalance, QueuedDeposit, Sweep, event::CreditedDeposit};
    use base64::{Engine, engine::general_purpose::STANDARD};
    use cksol_types::DepositSolId;
    use icrc_ledger_types::icrc1::account::Account;
    use serde_json::json;
    use sol_rpc_types::Lamport;
    use solana_address::{Address, address};
    use solana_message::Message;
    use solana_transaction::{Transaction, versioned::VersionedTransaction};
    use solana_transaction_status_client_types::{
        EncodedConfirmedTransactionWithStatusMeta, EncodedTransaction, TransactionBinaryEncoding,
        UiTransactionError, UiTransactionStatusMeta,
    };

    pub const MINTER_ADDRESS: Address = address!("5yazYQT1Kwm3jEjMp58J5329gzbxA232fnPajemCeKbL");

    /// Caches the master public key whose children sign [`derived_outcome`], so a sweep
    /// of [`derived_deposits`] can flow through the event-sourced state.
    pub fn init_master_key() {
        crate::state::mutate_state(|s| s.cache_minter_public_key(master_key()));
    }

    pub fn master_key() -> crate::state::SchnorrPublicKey {
        crate::state::SchnorrPublicKey {
            public_key: master_private_key().public_key(),
            chain_code: MASTER_CHAIN_CODE,
        }
    }

    pub fn minter_main_address() -> Address {
        crate::address::minter_address(&master_key())
    }

    const MASTER_CHAIN_CODE: [u8; 32] = [7; 32];

    fn master_private_key() -> ic_ed25519::PrivateKey {
        ic_ed25519::PrivateKey::generate_from_seed(b"devnet sweep master key")
    }

    fn sign_as(account: &Account, message: &[u8]) -> solana_signature::Signature {
        let path = ic_ed25519::DerivationPath::new(
            crate::address::derivation_path(account)
                .into_iter()
                .map(ic_ed25519::DerivationIndex)
                .collect(),
        );
        let (child, _chain_code) =
            master_private_key().derive_subkey_with_chain_code(&path, &MASTER_CHAIN_CODE);
        solana_signature::Signature::from(child.sign_message(message))
    }

    /// The deposits of the devnet sweep with their addresses derived from
    /// [`master_key`], so that their sweep satisfies the queued-address check
    /// and its rebuilt message can be signed by [`sign_as`].
    pub fn derived_deposits() -> Vec<(DepositSolId, QueuedDeposit)> {
        DEPOSITS
            .into_iter()
            .enumerate()
            .map(|(index, (_, balance))| {
                let account = account(index + 1);
                (
                    index as DepositSolId,
                    QueuedDeposit {
                        account,
                        address: crate::address::account_address(&master_key(), &account),
                        balance: DepositBalance::new(balance)
                            .expect("BUG: the balance covers the rent exemption threshold"),
                    },
                )
            })
            .collect()
    }

    /// A deposit queued while [`master_key`] is recorded, so its address must be
    /// derived from that key.
    pub fn fresh_deposit(account: Account, sweepable_amount: Lamport) -> QueuedDeposit {
        QueuedDeposit {
            account,
            address: crate::address::account_address(&master_key(), &account),
            balance: DepositBalance::new(
                sweepable_amount + crate::constants::RENT_EXEMPTION_THRESHOLD,
            )
            .expect("BUG: the balance covers the rent exemption threshold"),
        }
    }

    pub fn derived_sweep() -> Sweep {
        Sweep::plan(derived_deposits(), minter_main_address())
    }

    /// The first signature of the transaction of [`derived_outcome`], under which the
    /// sweep is submitted and queried.
    pub fn transaction_signature() -> solana_signature::Signature {
        derived_transaction().signatures[0]
    }

    fn derived_transaction() -> VersionedTransaction {
        let real = outcome()
            .transaction
            .transaction
            .decode()
            .expect("BUG: the devnet transaction should decode");
        let message = derived_sweep().sweep_message(*real.message.recent_blockhash());
        let address_to_account: std::collections::BTreeMap<Address, Account> = derived_deposits()
            .into_iter()
            .map(|(_, deposit)| (deposit.address, deposit.account))
            .collect();
        let message_bytes = message.serialize();
        let signatures = message.account_keys[..message.header.num_required_signatures as usize]
            .iter()
            .map(|signer| {
                sign_as(
                    address_to_account
                        .get(signer)
                        .expect("BUG: every signer of the sweep is a deposit address"),
                    &message_bytes,
                )
            })
            .collect();
        VersionedTransaction {
            signatures,
            message: solana_message::VersionedMessage::Legacy(message),
        }
    }

    /// The devnet outcome rewritten over [`derived_deposits`]: the message is rebuilt
    /// from the plan with the recorded blockhash and signed by the deposit keys, and
    /// the balances are remapped to the new account order, while the fee and amounts
    /// stay the devnet ones.
    pub fn derived_outcome() -> EncodedConfirmedTransactionWithStatusMeta {
        let mut outcome = outcome();
        outcome.transaction.transaction = EncodedTransaction::Binary(
            STANDARD.encode(
                bincode::serialize(&derived_transaction())
                    .expect("BUG: the transaction should serialize"),
            ),
            TransactionBinaryEncoding::Base64,
        );
        for (deposit_id, deposit) in derived_deposits() {
            set_balances(
                &mut outcome,
                deposit.address,
                DEPOSITS[deposit_id as usize].1,
                crate::constants::RENT_EXEMPTION_THRESHOLD,
            );
        }
        set_balances(&mut outcome, minter_main_address(), 0, AMOUNT_RECEIVED);
        set_balances(&mut outcome, solana_system_interface::program::ID, 1, 1);
        outcome
    }
    pub const FEE: Lamport = 20_000;
    pub const AMOUNT_RECEIVED: Lamport = 2_396_416_480;
    /// The deposit addresses with their balances when queued, the fee payer first.
    pub const DEPOSITS: [(Address, Lamport); 4] = [
        (
            address!("FkEEvAwZNvSziMAkmt13z2L9Loy4MjJE2qDbRserzt5Z"),
            1_300_000_000,
        ),
        (
            address!("5tEJDWwGGG54bv2xzrphSieXSAYjLkxzdMEPGe53JaTj"),
            600_000_000,
        ),
        (
            address!("6rXWHNpqRuNuGdUvJbRdV9wgRRggBVSEeCsnqJYDh7K9"),
            300_000_000,
        ),
        (
            address!("AwgbdmwCfwc6qaZAVH2K5juuP7HL21kotkVreWym6Eq4"),
            200_000_000,
        ),
    ];
    /// The sweepable amount of each deposit minus its share of the fee.
    pub const AMOUNTS_TO_MINT: [Lamport; 4] =
        [1_299_104_120, 599_104_120, 299_104_120, 199_104_120];

    /// The deposits of the sweep under the ids `0..`, owned by the accounts `1..`.
    pub fn deposits() -> Vec<(DepositSolId, QueuedDeposit)> {
        DEPOSITS
            .into_iter()
            .enumerate()
            .map(|(index, (address, balance))| {
                (
                    index as DepositSolId,
                    QueuedDeposit {
                        account: account(index + 1),
                        address,
                        balance: DepositBalance::new(balance)
                            .expect("BUG: the balance covers the rent exemption threshold"),
                    },
                )
            })
            .collect()
    }

    pub fn sweep() -> Sweep {
        Sweep::plan(deposits(), MINTER_ADDRESS)
    }

    pub fn mints() -> Vec<CreditedDeposit> {
        AMOUNTS_TO_MINT
            .into_iter()
            .enumerate()
            .map(|(index, amount_to_mint)| CreditedDeposit {
                deposit_id: index as DepositSolId,
                amount_to_mint,
            })
            .collect()
    }

    pub fn outcome() -> EncodedConfirmedTransactionWithStatusMeta {
        serde_json::from_value(json!({
          "blockTime": 1790862585u64,
          "meta": {
            "computeUnitsConsumed": 600,
            "costUnits": 5000,
            "err": null,
            "fee": 20000,
            "innerInstructions": [],
            "loadedAddresses": {
              "readonly": [],
              "writable": []
            },
            "logMessages": [
              "Program 11111111111111111111111111111111 invoke [1]",
              "Program 11111111111111111111111111111111 success",
              "Program 11111111111111111111111111111111 invoke [1]",
              "Program 11111111111111111111111111111111 success",
              "Program 11111111111111111111111111111111 invoke [1]",
              "Program 11111111111111111111111111111111 success",
              "Program 11111111111111111111111111111111 invoke [1]",
              "Program 11111111111111111111111111111111 success"
            ],
            "postBalances": [
              890880u64,
              890880u64,
              890880u64,
              890880u64,
              2396416480u64,
              1u64
            ],
            "postTokenBalances": [],
            "preBalances": [
              1300000000u64,
              600000000u64,
              300000000u64,
              200000000u64,
              0u64,
              1u64
            ],
            "preTokenBalances": [],
            "rewards": [],
            "status": {
              "Ok": null
            }
          },
          "slot": 506293219u64,
          "transaction": [
            "BM/CkSfTLKj3DZWIrizfXnF5M3TkH8RSgdpeALtgF5kmrNP8FVTpK7Uf75LMGvYrva7zdm5QGoyS2Ueu/BiZ5wvlpcLP4yd/Dq2R6/jnNyCzL3z2+K8WjFpXVEQmm0Mmpg0w20rlMMTRkeuTGNDEKkHu6wGivLwDOGlmEFlFKHEC01HYieqayyULa8H7zVMIxZ2tQgsSf/ZVEuK4Z+G3XaJ6DyhniFeZX3Z/sWLFiCnVBICA6yx3wueXGPbKuXSLC63gH2PkNasWcMp9T9PfKEraBwgu/eQDV1XAd+IzOmzgBbcpLjfsPt9O75qIwG2r0murfk/UMKATNvuQTQC8XgYEAAEG2xaP7ReiK7O/RyEMJK1y9oOUrRiIYixAgLbM81sLmMxIjmdbL31Vgaf52SlZWAP86sW6R0T6zA2hJjN7zYaxulb6YuCssGYBUTmCdb/38AL/yA3X2RJ8oD8BfyVye+uUk7tRk1tfdTVCsZXY9DqmOyL02mxVyqvO8TNExRK7+P9J7bU/SB+jWFVOOkEkrbUMHK3C368iWfar8UzS8WpUhwAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAE0o5/SLfZEhVop1j2SvWQVHZzfaoABQ2jhOLKuv+goIEBQIABAwCAAAA4IZuTQAAAAAFAgEEDAIAAAAArrUjAAAAAAUCAgQMAgAAAAAL1BEAAAAABQIDBAwCAAAAACreCwAAAAA=",
            "base64"
          ],
          "transactionIndex": 9,
          "version": "legacy"
        }))
        .expect("BUG: the getTransaction result should deserialize")
    }

    pub fn meta(
        outcome: &mut EncodedConfirmedTransactionWithStatusMeta,
    ) -> &mut UiTransactionStatusMeta {
        outcome
            .transaction
            .meta
            .as_mut()
            .expect("BUG: the devnet response has a meta field")
    }

    pub fn balances(
        outcome: &EncodedConfirmedTransactionWithStatusMeta,
        address: Address,
    ) -> (Lamport, Lamport) {
        let index = account_index(outcome, address);
        let meta = outcome
            .transaction
            .meta
            .as_ref()
            .expect("BUG: the devnet response has a meta field");
        (meta.pre_balances[index], meta.post_balances[index])
    }

    pub fn set_balances(
        outcome: &mut EncodedConfirmedTransactionWithStatusMeta,
        address: Address,
        pre: Lamport,
        post: Lamport,
    ) {
        let index = account_index(outcome, address);
        let meta = meta(outcome);
        meta.pre_balances[index] = pre;
        meta.post_balances[index] = post;
    }

    pub fn set_error(
        outcome: &mut EncodedConfirmedTransactionWithStatusMeta,
        error: UiTransactionError,
    ) {
        let meta = meta(outcome);
        meta.err = Some(error.clone());
        meta.status = Err(error);
    }

    pub fn set_message(outcome: &mut EncodedConfirmedTransactionWithStatusMeta, message: Message) {
        let transaction = VersionedTransaction::from(Transaction::new_unsigned(message));
        let encoded = STANDARD.encode(
            bincode::serialize(&transaction)
                .expect("BUG: serializing the transaction should succeed"),
        );
        outcome.transaction.transaction =
            EncodedTransaction::Binary(encoded, TransactionBinaryEncoding::Base64);
    }

    pub fn corrupt_transaction(outcome: &mut EncodedConfirmedTransactionWithStatusMeta) {
        match &mut outcome.transaction.transaction {
            EncodedTransaction::Binary(blob, _) => blob.push('!'),
            other => panic!("BUG: expected a binary transaction, got {other:?}"),
        }
    }

    fn account_index(
        outcome: &EncodedConfirmedTransactionWithStatusMeta,
        address: Address,
    ) -> usize {
        outcome
            .transaction
            .transaction
            .decode()
            .expect("BUG: the transaction should decode")
            .message
            .static_account_keys()
            .iter()
            .position(|key| *key == address)
            .unwrap_or_else(|| panic!("BUG: {address} is not an account of the transaction"))
    }
}

/// Helpers for constructing state transitions via [`process_event`] in tests.
///
/// All helpers operate on the global thread-local state via [`mutate_state`].
pub mod events {
    use super::{
        DEFAULT_BLOCK_HEIGHT, MINTER_ADDRESS, NONCE_ACCOUNT, WITHDRAWAL_FEE, queued_deposit,
        queued_deposit_of, runtime::TestCanisterRuntime, withdrawal_batch_message,
    };
    use crate::deposit::sweep::deposit_status;
    use crate::{
        numeric::{LedgerBurnIndex, LedgerMintIndex},
        rpc::BlockHeight,
        state::{
            QueuedDeposit, Sweep,
            audit::process_event,
            event::{EventType, Signer, TransactionPurpose, WithdrawalRequest},
            mutate_state, read_state,
        },
    };
    use cksol_types::{DepositSolId, DepositSolStatus};
    use icrc_ledger_types::icrc1::account::Account;
    use sol_rpc_types::Lamport;
    use solana_address::Address;
    use solana_signature::Signature;

    /// The runtime is only used by [`process_event`] to supply timestamps
    /// for the state transition and for the event log entry.
    fn runtime() -> TestCanisterRuntime {
        TestCanisterRuntime::new().add_times([0, 0])
    }

    pub fn queue_deposit(
        deposit_id: DepositSolId,
        account: Account,
        sweepable_amount: Lamport,
    ) -> DepositSolStatus {
        queue(deposit_id, queued_deposit_of(account, sweepable_amount))
    }

    /// Queues `N` distinct deposits under the ids `0..N` and returns them in that order.
    pub fn queue_deposits<const N: usize>() -> [QueuedDeposit; N] {
        std::array::from_fn(|index| {
            let deposit = queued_deposit(index as DepositSolId);
            queue(index as DepositSolId, deposit);
            deposit
        })
    }

    pub fn queue(deposit_id: DepositSolId, deposit: QueuedDeposit) -> DepositSolStatus {
        mutate_state(|state| {
            process_event(
                state,
                EventType::QueuedDeposit {
                    deposit_id,
                    account: deposit.account,
                    address: deposit.address,
                    balance: deposit.balance,
                },
                &runtime(),
            )
        });
        deposit_status(deposit_id)
    }

    /// Submits a sweep of the given queued deposits to [`MINTER_ADDRESS`], signed by their
    /// accounts in the given order.
    pub fn submit_sweep(signature: Signature, deposit_ids: Vec<DepositSolId>) {
        submit_sweep_to(signature, deposit_ids, MINTER_ADDRESS)
    }

    pub fn submit_sweep_at_height(
        signature: Signature,
        deposit_ids: Vec<DepositSolId>,
        block_height: BlockHeight,
    ) {
        submit_sweep_to_at_height(signature, deposit_ids, MINTER_ADDRESS, block_height)
    }

    pub fn submit_sweep_to(
        signature: Signature,
        deposit_ids: Vec<DepositSolId>,
        minter_address: Address,
    ) {
        submit_sweep_to_at_height(signature, deposit_ids, minter_address, DEFAULT_BLOCK_HEIGHT)
    }

    fn submit_sweep_to_at_height(
        signature: Signature,
        deposit_ids: Vec<DepositSolId>,
        minter_address: Address,
        block_height: BlockHeight,
    ) {
        let deposits: Vec<_> = read_state(|state| {
            deposit_ids
                .iter()
                .filter_map(|deposit_id| {
                    let deposit = state.deposits().queued().get(deposit_id)?;
                    Some((*deposit_id, *deposit))
                })
                .collect()
        });
        let signers = deposits
            .iter()
            .map(|(_, deposit)| Signer::Account(deposit.account))
            .collect();
        mutate_state(|state| {
            process_event(
                state,
                EventType::SubmittedTransaction {
                    signature,
                    message: Sweep::plan(deposits, minter_address)
                        .sweep_message(solana_hash::Hash::default())
                        .into(),
                    signers,
                    purpose: TransactionPurpose::SweepDeposit {
                        deposit_ids,
                        block_height,
                    },
                },
                &runtime(),
            )
        });
    }

    pub fn credit_sweep(signature: Signature, amount_received: Lamport) {
        credit_sweep_at(signature, amount_received, 0);
    }

    pub fn credit_sweep_at(signature: Signature, amount_received: Lamport, timestamp: u64) {
        let mints = read_state(|state| {
            state
                .deposits()
                .finalized()
                .get(&signature)
                .expect("BUG: no finalized sweep with the given signature")
                .mints()
        });
        mutate_state(|state| {
            process_event(
                state,
                EventType::CreditedSweep {
                    signature,
                    amount_received,
                    mints,
                },
                &TestCanisterRuntime::new().add_times([timestamp, timestamp]),
            )
        });
    }

    pub fn mint_swept_deposit(deposit_id: DepositSolId, mint_block_index: u64) {
        mutate_state(|state| {
            process_event(
                state,
                EventType::MintedSweptDeposit {
                    deposit_id,
                    mint_block_index: LedgerMintIndex::from(mint_block_index),
                },
                &runtime(),
            )
        });
    }

    pub fn quarantine_pending_mint(deposit_id: DepositSolId) {
        mutate_state(|state| {
            process_event(
                state,
                EventType::QuarantinedPendingMint { deposit_id },
                &runtime(),
            )
        });
    }

    pub fn quarantine_sweep(signature: Signature) {
        mutate_state(|state| {
            process_event(state, EventType::QuarantinedSweep { signature }, &runtime())
        });
    }

    pub fn accept_withdrawal(account: Account, burn_index: u64, amount: Lamport) {
        accept_withdrawal_at(account, burn_index, amount, 0);
    }

    pub fn accept_withdrawal_at(
        account: Account,
        burn_index: u64,
        amount: Lamport,
        timestamp: u64,
    ) {
        mutate_state(|state| {
            process_event(
                state,
                EventType::AcceptedWithdrawalRequest(WithdrawalRequest {
                    account,
                    solana_address: [0u8; 32],
                    burn_block_index: LedgerBurnIndex::from(burn_index),
                    amount_to_transfer: amount - WITHDRAWAL_FEE,
                    burned_amount: amount,
                }),
                &TestCanisterRuntime::new().add_times([timestamp, timestamp]),
            )
        });
    }

    /// Records a `CreatedWithdrawalTransaction` for the given withdrawals, binding
    /// [`NONCE_ACCOUNT`] to `nonce_value`.
    pub fn create_withdrawal_batch_transaction(
        nonce_value: solana_hash::Hash,
        burn_indices: Vec<u64>,
    ) {
        create_withdrawal_batch_transaction_on(NONCE_ACCOUNT, nonce_value, burn_indices);
    }

    /// Records a `CreatedWithdrawalTransaction` for the given withdrawals, binding
    /// `nonce_account` to `nonce_value`.
    pub fn create_withdrawal_batch_transaction_on(
        nonce_account: Address,
        nonce_value: solana_hash::Hash,
        burn_indices: Vec<u64>,
    ) {
        mutate_state(|state| {
            process_event(
                state,
                EventType::CreatedWithdrawalTransaction {
                    burn_indices: burn_indices
                        .into_iter()
                        .map(LedgerBurnIndex::from)
                        .collect(),
                    nonce_account,
                    nonce_value,
                },
                &runtime(),
            )
        });
    }

    /// Records a `SubmittedTransaction` for the durable-nonce withdrawal
    /// transaction advancing [`NONCE_ACCOUNT`] with `nonce_value` and
    /// transferring the created withdrawal requests of `burn_indices`.
    pub fn submit_withdrawal_batch_transaction(
        signature: Signature,
        nonce_value: solana_hash::Hash,
        burn_indices: Vec<u64>,
    ) {
        let burn_indices: Vec<LedgerBurnIndex> = burn_indices
            .into_iter()
            .map(LedgerBurnIndex::from)
            .collect();
        let message = withdrawal_batch_message(
            NONCE_ACCOUNT,
            nonce_value,
            &created_withdrawal_transfers(&burn_indices),
        );
        mutate_state(|state| {
            process_event(
                state,
                EventType::SubmittedTransaction {
                    signature,
                    message: message.into(),
                    signers: vec![Signer::Minter],
                    purpose: TransactionPurpose::Withdrawal { burn_indices },
                },
                &runtime(),
            )
        });
    }

    /// Marks the given withdrawals as sent under `signature` by creating and
    /// submitting a withdrawal transaction bound to a dedicated nonce account
    /// derived from the signature, which is first added to the pool through an
    /// upgrade event, so that repeated calls never contend for one account.
    pub fn submit_withdrawal(signature: Signature, burn_indices: Vec<u64>) {
        let nonce_account = add_nonce_account_of(&signature);
        let nonce_value = nonce_value_of(&signature);
        let burn_indices: Vec<LedgerBurnIndex> = burn_indices
            .into_iter()
            .map(LedgerBurnIndex::from)
            .collect();
        mutate_state(|state| {
            process_event(
                state,
                EventType::CreatedWithdrawalTransaction {
                    burn_indices: burn_indices.clone(),
                    nonce_account,
                    nonce_value,
                },
                &runtime(),
            )
        });
        let message = withdrawal_batch_message(
            nonce_account,
            nonce_value,
            &created_withdrawal_transfers(&burn_indices),
        );
        mutate_state(|state| {
            process_event(
                state,
                EventType::SubmittedTransaction {
                    signature,
                    message: message.into(),
                    signers: vec![Signer::Minter],
                    purpose: TransactionPurpose::Withdrawal { burn_indices },
                },
                &runtime(),
            )
        });
    }

    fn created_withdrawal_transfers(burn_indices: &[LedgerBurnIndex]) -> Vec<(Address, Lamport)> {
        read_state(|state| {
            burn_indices
                .iter()
                .map(|burn_index| {
                    let request = &state.created_withdrawal_requests()[burn_index].request;
                    (
                        Address::from(request.solana_address),
                        request.amount_to_transfer,
                    )
                })
                .collect()
        })
    }

    fn add_nonce_account_of(signature: &Signature) -> Address {
        use sha2::Digest;
        let digest = sha2::Sha256::digest(signature.as_ref());
        let nonce_account = Address::from(<[u8; 32]>::from(digest));
        mutate_state(|state| {
            process_event(
                state,
                EventType::Upgrade(cksol_types_internal::UpgradeArgs {
                    nonce_accounts_to_add: Some(vec![nonce_account.to_string()]),
                    ..cksol_types_internal::UpgradeArgs::default()
                }),
                &runtime(),
            )
        });
        nonce_account
    }

    fn nonce_value_of(signature: &Signature) -> solana_hash::Hash {
        let bytes: [u8; 32] = signature.as_ref()[32..]
            .try_into()
            .expect("BUG: a signature holds exactly 64 bytes");
        solana_hash::Hash::from(bytes)
    }

    pub fn succeed_transaction(signature: Signature) {
        mutate_state(|state| {
            process_event(
                state,
                EventType::SucceededTransaction { signature },
                &runtime(),
            )
        });
    }

    pub fn fail_transaction(signature: Signature) {
        mutate_state(|state| {
            process_event(
                state,
                EventType::FailedTransaction { signature },
                &runtime(),
            )
        });
    }

    pub fn expire_transaction(signature: Signature) {
        mutate_state(|state| {
            process_event(
                state,
                EventType::ExpiredTransaction { signature },
                &runtime(),
            )
        });
    }
}

#[cfg(test)]
pub mod arb {
    use crate::{
        constants::{FEE_PER_SIGNATURE, RENT_EXEMPTION_THRESHOLD},
        numeric::{LedgerBurnIndex, LedgerMintIndex},
        rpc::BlockHeight,
        sol_transfer::MAX_SIGNATURES,
        state::{
            DepositBalance, QueuedDeposit,
            event::{
                CreditedDeposit, Event, EventType, Signer, TransactionPurpose, WithdrawalRequest,
            },
        },
    };
    use candid::Principal;
    use cksol_types::DepositSolId;
    use cksol_types_internal::{Ed25519KeyName, InitArgs, SolanaNetwork, UpgradeArgs};
    use ic_ed25519::{PrivateKey, PublicKey};
    use icrc_ledger_types::icrc1::account::Account;
    use proptest::prelude::{Just, Strategy, any, prop, prop_oneof};
    use solana_address::Address;
    use solana_message::{Hash, Instruction, Message};
    use solana_signature::Signature;
    use std::collections::BTreeMap;

    pub fn arb_principal() -> impl Strategy<Value = Principal> {
        prop::collection::vec(any::<u8>(), 0..=29).prop_map(|bytes| Principal::from_slice(&bytes))
    }

    pub fn arb_subaccount() -> impl Strategy<Value = Option<[u8; 32]>> {
        prop::option::of(any::<[u8; 32]>())
    }

    pub fn arb_account() -> impl Strategy<Value = Account> {
        (arb_principal(), arb_subaccount())
            .prop_map(|(owner, subaccount)| Account { owner, subaccount })
    }

    pub fn arb_signature() -> impl Strategy<Value = Signature> {
        any::<[u8; 64]>().prop_map(Signature::from)
    }

    pub fn arb_signer() -> impl Strategy<Value = Signer> {
        prop_oneof![
            Just(Signer::Minter),
            arb_account().prop_map(Signer::Account),
        ]
    }

    pub fn arb_block_height() -> impl Strategy<Value = BlockHeight> {
        any::<u64>().prop_map(BlockHeight::from)
    }

    pub fn arb_ledger_mint_index() -> impl Strategy<Value = LedgerMintIndex> {
        any::<u64>().prop_map(LedgerMintIndex::from)
    }

    pub fn arb_deposit_balance() -> impl Strategy<Value = DepositBalance> {
        (RENT_EXEMPTION_THRESHOLD..=u64::MAX).prop_map(|balance| {
            DepositBalance::new(balance)
                .expect("BUG: the balance covers the rent exemption threshold")
        })
    }

    /// A deposit whose sweepable amount covers the fee of a full sweep and whose balance,
    /// multiplied by the deposits of a full sweep, fits in lamports.
    pub fn arb_queued_deposit() -> impl Strategy<Value = QueuedDeposit> {
        const MIN_BALANCE: u64 = RENT_EXEMPTION_THRESHOLD + FEE_PER_SIGNATURE * MAX_SIGNATURES;
        const MAX_BALANCE: u64 = u64::MAX / MAX_SIGNATURES;
        (arb_account(), arb_address(), MIN_BALANCE..=MAX_BALANCE).prop_map(
            |(account, address, balance)| QueuedDeposit {
                account,
                address,
                balance: DepositBalance::new(balance)
                    .expect("BUG: the balance covers the rent exemption threshold"),
            },
        )
    }

    /// The deposits of one sweep: at least one, at most one per signature.
    pub fn arb_sweep_deposits() -> impl Strategy<Value = BTreeMap<DepositSolId, QueuedDeposit>> {
        prop::collection::btree_map(
            any::<DepositSolId>(),
            arb_queued_deposit(),
            1..=MAX_SIGNATURES as usize,
        )
    }

    pub fn arb_address() -> impl Strategy<Value = Address> {
        any::<[u8; 32]>().prop_map(Address::from)
    }

    pub fn arb_ed25519_public_key() -> impl Strategy<Value = PublicKey> {
        any::<[u8; 32]>().prop_map(|seed| PrivateKey::generate_from_seed(&seed).public_key())
    }

    pub fn arb_hash() -> impl Strategy<Value = Hash> {
        any::<[u8; 32]>().prop_map(Hash::from)
    }

    pub fn arb_instruction() -> impl Strategy<Value = Instruction> {
        (
            arb_address(),
            prop::collection::vec(arb_address(), 0..5),
            prop::collection::vec(any::<u8>(), 0..32),
        )
            .prop_map(|(program_id, accounts, data)| {
                Instruction::new_with_bytes(
                    program_id,
                    &data,
                    accounts
                        .into_iter()
                        .map(|a| solana_message::AccountMeta::new(a, false))
                        .collect(),
                )
            })
    }

    pub fn arb_message() -> impl Strategy<Value = Message> {
        (
            prop::collection::vec(arb_instruction(), 1..10),
            prop::option::of(arb_address()),
            arb_hash(),
        )
            .prop_map(|(instructions, maybe_payer, blockhash)| {
                Message::new_with_blockhash(&instructions, maybe_payer.as_ref(), &blockhash)
            })
    }

    pub fn arb_ed25519_key_name() -> impl Strategy<Value = Ed25519KeyName> {
        prop_oneof![
            Just(Ed25519KeyName::LocalDevelopment),
            Just(Ed25519KeyName::MainnetTestKey1),
            Just(Ed25519KeyName::MainnetProdKey1),
        ]
    }

    pub fn arb_solana_network() -> impl Strategy<Value = SolanaNetwork> {
        prop_oneof![
            Just(SolanaNetwork::Mainnet),
            Just(SolanaNetwork::Devnet),
            Just(SolanaNetwork::Testnet),
        ]
    }

    pub fn arb_init_args() -> impl Strategy<Value = InitArgs> {
        (
            arb_principal(),
            arb_principal(),
            arb_ed25519_key_name(),
            any::<u64>(),
            any::<u64>(),
            any::<u64>(),
            any::<u64>(),
            arb_solana_network(),
            (any::<u64>(), arb_nonce_accounts()),
        )
            .prop_map(
                |(
                    sol_rpc_canister_id,
                    ledger_canister_id,
                    master_key_name,
                    minimum_withdrawal_amount,
                    minimum_deposit_amount,
                    withdrawal_fee,
                    deposit_sol_required_cycles,
                    solana_network,
                    (deposit_sol_fee, nonce_accounts),
                )| {
                    InitArgs {
                        sol_rpc_canister_id,
                        ledger_canister_id,
                        master_key_name,
                        minimum_withdrawal_amount,
                        minimum_deposit_amount,
                        withdrawal_fee,
                        deposit_sol_required_cycles,
                        solana_network,
                        deposit_sol_fee,
                        nonce_accounts,
                    }
                },
            )
    }

    fn arb_nonce_accounts() -> impl Strategy<Value = Vec<String>> {
        prop::collection::vec(arb_address().prop_map(|address| address.to_string()), 0..5)
    }

    pub fn arb_upgrade_args() -> impl Strategy<Value = UpgradeArgs> {
        (
            prop::option::of(arb_principal()),
            prop::option::of(any::<u64>()),
            prop::option::of(any::<u64>()),
            prop::option::of(any::<u64>()),
            prop::option::of(any::<u64>()),
            prop::option::of(any::<u64>()),
            prop::option::of(arb_nonce_accounts()),
        )
            .prop_map(
                |(
                    sol_rpc_canister_id,
                    minimum_withdrawal_amount,
                    minimum_deposit_amount,
                    withdrawal_fee,
                    deposit_sol_required_cycles,
                    deposit_sol_fee,
                    nonce_accounts_to_add,
                )| UpgradeArgs {
                    sol_rpc_canister_id,
                    minimum_withdrawal_amount,
                    minimum_deposit_amount,
                    withdrawal_fee,
                    deposit_sol_required_cycles,
                    deposit_sol_fee,
                    nonce_accounts_to_add,
                },
            )
    }

    pub fn arb_ledger_burn_index() -> impl Strategy<Value = LedgerBurnIndex> {
        any::<u64>().prop_map(LedgerBurnIndex::from)
    }

    pub fn arb_withdrawal_request() -> impl Strategy<Value = WithdrawalRequest> {
        (
            arb_account(),
            any::<[u8; 32]>(),
            arb_ledger_burn_index(),
            any::<u64>(),
            any::<u64>(),
        )
            .prop_map(
                |(account, solana_address, burn_block_index, burned_amount, amount_to_transfer)| {
                    WithdrawalRequest {
                        account,
                        solana_address,
                        burn_block_index,
                        burned_amount,
                        amount_to_transfer,
                    }
                },
            )
    }

    pub fn arb_event_type() -> impl Strategy<Value = EventType> {
        prop_oneof![
            arb_init_args().prop_map(EventType::Init),
            arb_upgrade_args().prop_map(EventType::Upgrade),
            arb_withdrawal_request().prop_map(EventType::AcceptedWithdrawalRequest),
            (
                arb_signature(),
                arb_message(),
                prop::collection::vec(arb_signer(), 1..10),
                arb_transaction_purpose(),
            )
                .prop_map(|(signature, message, signers, purpose)| {
                    EventType::SubmittedTransaction {
                        signature,
                        message: message.into(),
                        signers,
                        purpose,
                    }
                }),
            arb_signature().prop_map(|signature| EventType::SucceededTransaction { signature }),
            arb_signature().prop_map(|signature| EventType::FailedTransaction { signature }),
            arb_signature().prop_map(|signature| EventType::ExpiredTransaction { signature }),
            (
                any::<u64>(),
                arb_account(),
                arb_address(),
                arb_deposit_balance(),
            )
                .prop_map(|(deposit_id, account, address, balance)| {
                    EventType::QueuedDeposit {
                        deposit_id,
                        account,
                        address,
                        balance,
                    }
                },),
            (
                arb_signature(),
                any::<u64>(),
                prop::collection::vec(arb_credited_deposit(), 0..10)
            )
                .prop_map(|(signature, amount_received, mints)| {
                    EventType::CreditedSweep {
                        signature,
                        amount_received,
                        mints,
                    }
                }),
            arb_signature().prop_map(|signature| EventType::QuarantinedSweep { signature }),
            (arb_ed25519_public_key(), any::<[u8; 32]>()).prop_map(|(public_key, chain_code)| {
                EventType::MinterPublicKeyFetched {
                    public_key,
                    chain_code,
                }
            }),
            (any::<u64>(), arb_ledger_mint_index()).prop_map(|(deposit_id, mint_block_index)| {
                EventType::MintedSweptDeposit {
                    deposit_id,
                    mint_block_index,
                }
            }),
            any::<u64>().prop_map(|deposit_id| EventType::QuarantinedPendingMint { deposit_id }),
            (
                prop::collection::vec(arb_ledger_burn_index(), 1..10),
                arb_address(),
                arb_hash(),
            )
                .prop_map(|(burn_indices, nonce_account, nonce_value)| {
                    EventType::CreatedWithdrawalTransaction {
                        burn_indices,
                        nonce_account,
                        nonce_value,
                    }
                }),
        ]
    }

    fn arb_credited_deposit() -> impl Strategy<Value = CreditedDeposit> {
        (any::<u64>(), any::<u64>()).prop_map(|(deposit_id, amount_to_mint)| CreditedDeposit {
            deposit_id,
            amount_to_mint,
        })
    }

    fn arb_transaction_purpose() -> impl Strategy<Value = TransactionPurpose> {
        prop_oneof![
            (
                prop::collection::vec(any::<u64>(), 1..10),
                arb_block_height()
            )
                .prop_map(|(deposit_ids, block_height)| {
                    TransactionPurpose::SweepDeposit {
                        deposit_ids,
                        block_height,
                    }
                }),
            prop::collection::vec(arb_ledger_burn_index(), 1..10)
                .prop_map(|burn_indices| TransactionPurpose::Withdrawal { burn_indices }),
        ]
    }

    pub fn arb_event() -> impl Strategy<Value = Event> {
        (any::<u64>(), arb_event_type())
            .prop_map(|(timestamp, payload)| Event { timestamp, payload })
    }
}

pub mod deposit {
    use super::*;

    pub const DEPOSIT_AMOUNT: Lamport = 500_000_000;
    pub const DEPOSIT_ADDRESS: Address = address!("BVH7GZXRdqyZLSLBS4cm1Yom8Yvekw6ytgSFz9y9on4e");
    pub const DEPOSITOR_PRINCIPAL: Principal = Principal::from_slice(&[0x9d, 0xf7, 0x02]);
    pub const DEPOSITOR_ACCOUNT: Account = Account {
        owner: DEPOSITOR_PRINCIPAL,
        subaccount: None,
    };

    // Legacy (non-versioned) deposit transaction.
    // https://explorer.solana.com/tx/49aFRmEtgnVN3UetkKHJbz3ZMcDY6pgS9oDoN4Y4NQYfHSx4nsDsx3PSKubxfmY69URcosJj3CWu4aypeddduZYX?cluster=devnet
    pub fn legacy_deposit_transaction_signature() -> solana_signature::Signature {
        const SIGNATURE: &str = "49aFRmEtgnVN3UetkKHJbz3ZMcDY6pgS9oDoN4Y4NQYfHSx4nsDsx3PSKubxfmY69URcosJj3CWu4aypeddduZYX";
        solana_signature::Signature::from_str(SIGNATURE).unwrap()
    }

    // Legacy (non-versioned) 0.5 SOL transfer to DEPOSITOR_ACCOUNT's deposit address (BVH7GZXRdqyZLSLBS4cm1Yom8Yvekw6ytgSFz9y9on4e).
    // https://explorer.solana.com/tx/49aFRmEtgnVN3UetkKHJbz3ZMcDY6pgS9oDoN4Y4NQYfHSx4nsDsx3PSKubxfmY69URcosJj3CWu4aypeddduZYX?cluster=devnet
    pub fn legacy_deposit_transaction() -> EncodedConfirmedTransactionWithStatusMeta {
        const ENCODED_DEPOSIT_TRANSACTION: &str = "AZ1xufshIEi/hzGnwqjbgjUqDzcH3dfZQs3hZUbR8iHESSc+4eGeOwll0PMlDtORri5YQi433FjgQ5YK138CXQQBAAEDIg5JU11WGypQAKfOpxcE0+UIiKney1G6hf+6GRXcmseb01hqfWVQEn6n64lX4Uby5n5lTlmSpsWgEH1gv7LbVwAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA/S7SHgiiNOkFs7RGKc0VhLBrkHbCp47AK4FytcYYlDgBAgIAAQwCAAAAAGXNHQAAAAA=";
        EncodedConfirmedTransactionWithStatusMeta {
            slot: 443421331,
            transaction: EncodedTransactionWithStatusMeta {
                transaction: EncodedTransaction::Binary(
                    ENCODED_DEPOSIT_TRANSACTION.to_string(),
                    TransactionBinaryEncoding::Base64,
                ),
                meta: Some(UiTransactionStatusMeta {
                    compute_units_consumed: OptionSerializer::Some(150),
                    cost_units: OptionSerializer::Some(1481),
                    err: None,
                    fee: 5000,
                    inner_instructions: OptionSerializer::Some(vec![]),
                    loaded_addresses: OptionSerializer::Some(UiLoadedAddresses {
                        writable: vec![],
                        readonly: vec![],
                    }),
                    log_messages: OptionSerializer::Some(vec![
                        "Program 11111111111111111111111111111111 invoke [1]".to_string(),
                        "Program 11111111111111111111111111111111 success".to_string(),
                    ]),
                    post_balances: vec![895811440, 500000000, 1],
                    post_token_balances: OptionSerializer::Some(vec![]),
                    pre_balances: vec![1395816440, 0, 1],
                    pre_token_balances: OptionSerializer::Some(vec![]),
                    rewards: OptionSerializer::None,
                    status: Ok(()),
                    return_data: OptionSerializer::Skip,
                }),
                version: None,
            },
            block_time: Some(1771582425),
        }
    }
}

#[derive(Debug, PartialEq)]
pub struct EventsAssert(VecDeque<Event>);

impl EventsAssert {
    pub fn from_recorded() -> Self {
        Self(with_event_iter(|events| events.collect()))
    }

    pub fn assert_no_events_recorded() {
        Self::from_recorded().assert_no_more_events();
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn expect_event<F>(mut self, check: F) -> Self
    where
        F: Fn(EventType),
    {
        let event = self.0.pop_front().expect("No more events!");
        check(event.payload);
        self
    }

    pub fn expect_event_eq(mut self, expected: EventType) -> Self {
        let event = self.0.pop_front().expect("No more events!");
        assert_eq!(event.payload, expected);
        self
    }

    /// Asserts that `expected` appears exactly once, removes it, and returns the rest.
    pub fn expect_contains_event_eq(mut self, expected: EventType) -> Self {
        let pos = self
            .0
            .iter()
            .position(|event| event.payload == expected)
            .unwrap_or_else(|| {
                panic!("Expected to find event {expected:?} but it was not recorded")
            });
        self.0.remove(pos);
        assert!(
            !self.0.iter().any(|event| event.payload == expected),
            "Expected exactly 1 occurrence of {expected:?}, found more"
        );
        self
    }

    pub fn contains_event(&self, expected: &EventType) -> bool {
        self.0.iter().any(|event| &event.payload == expected)
    }

    pub fn assert_no_more_events(&self) {
        assert!(self.0.is_empty());
    }
}
