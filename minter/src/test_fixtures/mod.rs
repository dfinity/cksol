use crate::{
    address::{MINTER_DERIVATION_PATH, account_address, derivation_path},
    constants::RENT_EXEMPTION_THRESHOLD,
    rpc::BlockHeight,
    state::{
        DepositBalance, QueuedDeposit, SchnorrPublicKey, State, Sweep,
        event::{Event, EventType, VersionedMessage},
        init_once_state, mutate_state,
    },
    storage::with_event_iter,
};
use candid::Principal;
use cksol_types::DepositSolId;
use cksol_types_internal::{Ed25519KeyName, InitArgs, SolanaNetwork};
use ic_cdk_management_canister::SchnorrPublicKeyResult;
use ic_ed25519::{PocketIcMasterPublicKeyId, PublicKey};
use icrc_ledger_types::icrc1::account::Account;
use sol_rpc_types::{Lamport, MultiRpcResult};
use solana_address::{Address, address};
use solana_transaction_status_client_types::{
    EncodedConfirmedTransactionWithStatusMeta, EncodedTransaction,
    EncodedTransactionWithStatusMeta, TransactionBinaryEncoding, UiLoadedAddresses,
    UiTransactionStatusMeta, option_serializer::OptionSerializer,
};
use std::{collections::VecDeque, str::FromStr};

pub mod runtime;
pub mod signer;
mod stubs;
#[cfg(test)]
mod tests;

pub type GetTransactionResult =
    MultiRpcResult<Option<sol_rpc_types::EncodedConfirmedTransactionWithStatusMeta>>;

pub const BLOCK_INDEX: u64 = 98763_u64;
pub const AUTOMATED_DEPOSIT_FEE: Lamport = 10_000_000; // 0.01 SOL
pub const DEPOSIT_CONSOLIDATION_FEE: u128 = 10_000_000_000; // 0.01T cycles
pub const WITHDRAWAL_FEE: Lamport = 1_000_000; // 0.001 SOL
pub const MINIMUM_WITHDRAWAL_AMOUNT: Lamport = 2_000_000; // 0.002 SOL
pub const MINTER_ACCOUNT: Account = Account {
    owner: runtime::TEST_CANISTER_ID,
    subaccount: None,
};
/// The minter's main Solana address under the test master key: the raw master public key.
pub const MINTER_ADDRESS: Address = address!("Fkt68XQXBDDBGBNNjFh8GM27ffpZGmncUdDG19njnRvY");
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
        automated_deposit_fee: AUTOMATED_DEPOSIT_FEE,
        master_key_name: Ed25519KeyName::default(),
        minimum_withdrawal_amount: MINIMUM_WITHDRAWAL_AMOUNT,
        minimum_deposit_amount: MINIMUM_DEPOSIT_AMOUNT,
        withdrawal_fee: WITHDRAWAL_FEE,
        deposit_sol_required_cycles: DEPOSIT_SOL_REQUIRED_CYCLES as u64,
        solana_network: SolanaNetwork::Mainnet,
        deposit_consolidation_fee: DEPOSIT_CONSOLIDATION_FEE as u64,
    }
}

pub fn init_state() {
    init_state_with_args(valid_init_args());
}

pub fn init_state_with_args(init_args: InitArgs) {
    init_once_state(State::try_from(init_args).expect("Invalid init args"));
}

pub fn init_balance() {
    init_balance_to(u64::MAX / 2);
}

/// Credits the minter balance with exactly `amount` by sweeping a deposit of a
/// dedicated account through the whole flow, so the account is released again.
pub fn init_balance_to(amount: Lamport) {
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

fn schnorr_master_key() -> SchnorrPublicKey {
    SchnorrPublicKey {
        public_key: PublicKey::pocketic_key(PocketIcMasterPublicKeyId::Key1),
        chain_code: [1; 32],
    }
}

/// Returns a [`Signature`] unique for any `usize` index, derived from `i as u64` via le_bytes.
pub fn signature(i: usize) -> solana_signature::Signature {
    let mut bytes = [0u8; 64];
    bytes[..8].copy_from_slice(&(i as u64).to_le_bytes());
    solana_signature::Signature::from(bytes)
}

/// The block height used by fixtures whose test does not care about blockhash expiry.
pub const DEFAULT_BLOCK_HEIGHT: BlockHeight = BlockHeight::new(400_000_000);

/// Returns a [`ConfirmedBlock`] with a deterministic blockhash at
/// [`DEFAULT_BLOCK_HEIGHT`], for use in expected RPC responses.
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

/// A sweep of four deposits that the minter submitted on devnet as transaction
/// `59vLxkN5YGgBrHGTQMCrntNi7CrAxfkQxYek2v3hUgKfujgGtfkSDZZmFyVw6S59uTH2FEwWcvntPiEkdN5Ep5W2`,
/// with the `getTransaction` response the minter settles it against, and ways to deviate
/// from that response.
pub mod devnet_sweep {
    use super::account;
    use crate::state::{DepositBalance, QueuedDeposit, Sweep, event::CreditedDeposit};
    use base64::{Engine, engine::general_purpose::STANDARD};
    use cksol_types::DepositSolId;
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
        DEFAULT_BLOCK_HEIGHT, MINTER_ADDRESS, WITHDRAWAL_FEE, queued_deposit, queued_deposit_of,
        runtime::TestCanisterRuntime,
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

    fn message() -> solana_message::Message {
        let payer = solana_address::Address::from([0x42; 32]);
        solana_message::Message::new_with_blockhash(
            &[],
            Some(&payer),
            &solana_message::Hash::default(),
        )
    }

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

    pub fn submit_sweep_to(
        signature: Signature,
        deposit_ids: Vec<DepositSolId>,
        minter_address: Address,
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
                    purpose: TransactionPurpose::SweepDeposits { deposit_ids },
                    block_height: DEFAULT_BLOCK_HEIGHT,
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

    pub fn submit_withdrawal(signature: Signature, burn_indices: Vec<u64>) {
        submit_withdrawal_at_height(signature, DEFAULT_BLOCK_HEIGHT, burn_indices);
    }

    pub fn submit_withdrawal_at_height(
        signature: Signature,
        block_height: BlockHeight,
        burn_indices: Vec<u64>,
    ) {
        mutate_state(|state| {
            process_event(
                state,
                EventType::SubmittedTransaction {
                    signature,
                    message: message().into(),
                    signers: vec![Signer::Minter],
                    purpose: TransactionPurpose::WithdrawSol {
                        burn_indices: burn_indices
                            .into_iter()
                            .map(LedgerBurnIndex::from)
                            .collect(),
                    },
                    block_height,
                },
                &runtime(),
            )
        });
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

    pub fn resubmit_transaction(old_signature: Signature, new_signature: Signature) {
        mutate_state(|state| {
            process_event(
                state,
                EventType::ResubmittedTransaction {
                    old_signature,
                    new_signature,
                    new_block_height: DEFAULT_BLOCK_HEIGHT,
                },
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
            any::<u64>(),
            arb_ed25519_key_name(),
            any::<u64>(),
            any::<u64>(),
            any::<u64>(),
            any::<u64>(),
            arb_solana_network(),
            any::<u64>(),
        )
            .prop_map(
                |(
                    sol_rpc_canister_id,
                    ledger_canister_id,
                    automated_deposit_fee,
                    master_key_name,
                    minimum_withdrawal_amount,
                    minimum_deposit_amount,
                    withdrawal_fee,
                    deposit_sol_required_cycles,
                    solana_network,
                    deposit_consolidation_fee,
                )| {
                    InitArgs {
                        sol_rpc_canister_id,
                        ledger_canister_id,
                        automated_deposit_fee,
                        master_key_name,
                        minimum_withdrawal_amount,
                        minimum_deposit_amount,
                        withdrawal_fee,
                        deposit_sol_required_cycles,
                        solana_network,
                        deposit_consolidation_fee,
                    }
                },
            )
    }

    pub fn arb_upgrade_args() -> impl Strategy<Value = UpgradeArgs> {
        (
            prop::option::of(arb_principal()),
            prop::option::of(any::<u64>()),
            prop::option::of(any::<u64>()),
            prop::option::of(any::<u64>()),
            prop::option::of(any::<u64>()),
            prop::option::of(any::<u64>()),
            prop::option::of(any::<u64>()),
        )
            .prop_map(
                |(
                    sol_rpc_canister_id,
                    automated_deposit_fee,
                    minimum_withdrawal_amount,
                    minimum_deposit_amount,
                    withdrawal_fee,
                    deposit_sol_required_cycles,
                    deposit_consolidation_fee,
                )| UpgradeArgs {
                    sol_rpc_canister_id,
                    automated_deposit_fee,
                    minimum_withdrawal_amount,
                    minimum_deposit_amount,
                    withdrawal_fee,
                    deposit_sol_required_cycles,
                    deposit_consolidation_fee,
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
                prop_oneof![
                    prop::collection::vec(arb_ledger_burn_index(), 1..10)
                        .prop_map(|burn_indices| TransactionPurpose::WithdrawSol { burn_indices }),
                    prop::collection::vec(any::<u64>(), 1..10)
                        .prop_map(|deposit_ids| TransactionPurpose::SweepDeposits { deposit_ids }),
                ],
                arb_block_height(),
            )
                .prop_map(|(signature, message, signers, purpose, block_height)| {
                    EventType::SubmittedTransaction {
                        signature,
                        message: message.into(),
                        signers,
                        purpose,
                        block_height,
                    }
                }),
            (arb_signature(), arb_signature(), arb_block_height(),).prop_map(
                |(old_signature, new_signature, new_block_height)| {
                    EventType::ResubmittedTransaction {
                        old_signature,
                        new_signature,
                        new_block_height,
                    }
                }
            ),
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
            (any::<u64>(), arb_ledger_mint_index()).prop_map(|(deposit_id, mint_block_index)| {
                EventType::MintedSweptDeposit {
                    deposit_id,
                    mint_block_index,
                }
            }),
            any::<u64>().prop_map(|deposit_id| EventType::QuarantinedPendingMint { deposit_id }),
        ]
    }

    fn arb_credited_deposit() -> impl Strategy<Value = CreditedDeposit> {
        (any::<u64>(), any::<u64>()).prop_map(|(deposit_id, amount_to_mint)| CreditedDeposit {
            deposit_id,
            amount_to_mint,
        })
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
