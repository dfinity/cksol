#[cfg(test)]
mod tests;

use crate::{
    ledger::client::LedgerClient,
    state::event::{AcceptedDepositEvent, MintedEvent},
};
use assert_matches::assert_matches;
use candid::Principal;
use cksol_types::DepositStatus;
use cksol_types_internal::{Ed25519KeyName, InitArgs, UpgradeArgs};
use ic_canister_runtime::Runtime;
use ic_ed25519::PublicKey;
use icrc_ledger_types::icrc1::account::Account;
use sol_rpc_client::SolRpcClient;
use sol_rpc_types::{ConsensusStrategy, Lamport, RpcSources, SolanaCluster};
use solana_signature::Signature;
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
};

pub mod audit;
pub mod event;

thread_local! {
    static STATE: RefCell<Option<State>> = RefCell::default();
}

pub fn read_state<R>(f: impl FnOnce(&State) -> R) -> R {
    STATE.with(|s| f(s.borrow().as_ref().expect("BUG: state is not initialized")))
}

pub fn init_once_state(state: State) {
    STATE.with(|s| {
        if s.borrow().is_some() {
            panic!("BUG: state is already initialized");
        }
        *s.borrow_mut() = Some(state);
    });
}

pub fn mutate_state<F, R>(f: F) -> R
where
    F: FnOnce(&mut State) -> R,
{
    STATE.with(|s| {
        f(s.borrow_mut()
            .as_mut()
            .expect("BUG: state is not initialized"))
    })
}

/// State of the minter.
///
/// # Design
///
/// The state is transient and not preserved across canister upgrades.
/// Relevant state changes are recorded in an append-only event log
/// (see [`crate::state::audit::process_event`]),
/// and replaying this log upon canister upgrade will re-create an equivalent state.
///
/// That means in particular:
/// * Methods mutating the state should generally not be accessible outside the state crate,
///   to ensure that the state is only mutating through events.
/// * Having public methods mutating the state may be acceptable for transient data (e.g. guards)
///   that do not need to be preserved across canister upgrades.
#[derive(Debug, PartialEq, Eq)]
pub struct State {
    minter_public_key: Option<SchnorrPublicKey>,
    master_key_name: Ed25519KeyName,
    ledger_canister_id: Principal,
    sol_rpc_canister_id: Principal,
    deposit_fee: Lamport,
    minimum_withdrawal_amount: Lamport,
    pending_update_balance_requests: BTreeSet<Account>,
    events_to_mint: BTreeMap<(Account, Signature), AcceptedDepositEvent>,
    minted_events: BTreeMap<(Account, Signature), MintedEvent>,
}

impl State {
    pub fn minter_public_key(&self) -> Option<&SchnorrPublicKey> {
        self.minter_public_key.as_ref()
    }

    /// Set the minter public key only once.
    ///
    /// This is expected to happen only when the minter was freshly installed or after a canister upgrade.
    ///
    /// # Panics
    /// This method will panic if the public key was already set
    pub fn set_once_minter_public_key(&mut self, public_key: SchnorrPublicKey) {
        if self.minter_public_key.is_some() {
            panic!("BUG: minter public key is already set")
        }
        self.minter_public_key = Some(public_key);
    }

    pub fn sol_rpc_canister_id(&self) -> Principal {
        self.sol_rpc_canister_id
    }

    pub fn ledger_canister_id(&self) -> Principal {
        self.ledger_canister_id
    }

    pub fn master_key_name(&self) -> Ed25519KeyName {
        self.master_key_name
    }

    pub fn deposit_fee(&self) -> u64 {
        self.deposit_fee
    }

    pub fn minimum_withdrawal_amount(&self) -> u64 {
        self.minimum_withdrawal_amount
    }

    pub fn events_to_mint(&self) -> &BTreeMap<(Account, Signature), AcceptedDepositEvent> {
        &self.events_to_mint
    }

    pub fn minted_events(&self) -> &BTreeMap<(Account, Signature), MintedEvent> {
        &self.minted_events
    }

    pub fn deposit_status(&self, deposit: &(Account, Signature)) -> Option<DepositStatus> {
        let maybe_deposit_event = self.events_to_mint().get(deposit);
        let maybe_mint_event = self.minted_events().get(deposit);

        match (maybe_deposit_event, maybe_mint_event) {
            (None, None) => None,
            (Some(deposit_event), None) => {
                Some(DepositStatus::Processing(deposit_event.signature.into()))
            }
            (None, Some(minted_event)) => Some(DepositStatus::Minted {
                block_index: *minted_event.mint_block_index.get(),
                minted_amount: minted_event.minted_amount,
                signature: minted_event.deposit_event.signature.into(),
            }),
            (Some(_), Some(_)) => panic!(
                "Found both event to mint and minted event for deposit with account {:?} and signature {:?}",
                deposit.0, deposit.1
            ),
        }
    }

    pub fn sol_rpc_client<R: Runtime>(&self, runtime: R) -> SolRpcClient<R> {
        // The maximum size of an HTTPs outcall response is 2MB:
        // https://docs.internetcomputer.org/references/ic-interface-spec#ic-http_request
        const MAX_RESPONSE_BYTES: u64 = 2_000_000;
        SolRpcClient::builder(runtime, self.sol_rpc_canister_id)
            .with_rpc_sources(RpcSources::Default(SolanaCluster::Mainnet))
            .with_response_size_estimate(MAX_RESPONSE_BYTES)
            .with_consensus_strategy(ConsensusStrategy::Threshold {
                min: 3,
                total: Some(4),
            })
            .build()
    }

    pub fn ledger_client<R: Runtime>(&self, runtime: R) -> LedgerClient<R> {
        LedgerClient::new(runtime, self.ledger_canister_id)
    }

    pub fn pending_update_balance_requests_mut(&mut self) -> &mut BTreeSet<Account> {
        &mut self.pending_update_balance_requests
    }

    fn upgrade(
        &mut self,
        UpgradeArgs {
            sol_rpc_canister_id,
            deposit_fee,
            minimum_withdrawal_amount,
        }: UpgradeArgs,
    ) -> Result<(), InvalidStateError> {
        if let Some(sol_rpc_canister_id) = sol_rpc_canister_id {
            self.sol_rpc_canister_id = sol_rpc_canister_id;
        }
        if let Some(deposit_fee) = deposit_fee {
            self.deposit_fee = deposit_fee;
        }
        if let Some(minimum_withdrawal_amount) = minimum_withdrawal_amount {
            self.minimum_withdrawal_amount = minimum_withdrawal_amount;
        }
        Ok(())
    }

    fn record_event_to_mint(&mut self, event: &AcceptedDepositEvent) {
        let account = event.account;
        let signature = event.signature;
        assert!(
            !self.events_to_mint.contains_key(&(account, signature)),
            "There must not be two different events to mint for the same account and signature"
        );
        assert!(!self.minted_events.contains_key(&(account, signature)));
        self.events_to_mint
            .insert((account, signature), event.clone());
    }

    fn record_successful_mint(&mut self, event: &MintedEvent) {
        let account = event.deposit_event.account;
        let signature = event.deposit_event.signature;
        assert_matches!(
            self.events_to_mint.remove(&(account, signature)),
            Some(_),
            "Attempted to mint ckSOL for an unknown event {:?}",
            event.deposit_event
        );
        assert_eq!(
            self.minted_events
                .insert((account, signature), event.clone()),
            None,
            "Attempted to mint ckSOL twice for the same event {:?}",
            event.deposit_event
        );
    }
}

#[derive(Debug)]
pub enum InvalidStateError {
    InvalidCanisterId(String),
}

impl TryFrom<InitArgs> for State {
    type Error = InvalidStateError;

    fn try_from(
        InitArgs {
            sol_rpc_canister_id,
            ledger_canister_id,
            deposit_fee,
            master_key_name,
            minimum_withdrawal_amount,
        }: InitArgs,
    ) -> Result<Self, Self::Error> {
        let canister_ids: BTreeSet<_> = [sol_rpc_canister_id, ledger_canister_id]
            .into_iter()
            .collect();
        if canister_ids.contains(&Principal::anonymous()) {
            return Err(InvalidStateError::InvalidCanisterId(
                "ERROR: anonymous principal is not accepted!".to_string(),
            ));
        }
        if canister_ids.len() < 2 {
            return Err(InvalidStateError::InvalidCanisterId(
                "ERROR: provided canister IDs are not distinct!".to_string(),
            ));
        }

        Ok(Self {
            minter_public_key: None,
            master_key_name,
            ledger_canister_id,
            sol_rpc_canister_id,
            deposit_fee,
            minimum_withdrawal_amount,
            pending_update_balance_requests: BTreeSet::new(),
            events_to_mint: BTreeMap::new(),
            minted_events: BTreeMap::new(),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SchnorrPublicKey {
    pub public_key: PublicKey,
    pub chain_code: [u8; 32],
}
