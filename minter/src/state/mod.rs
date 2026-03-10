use crate::{
    ledger::client::LedgerClient,
    numeric::{LedgerBurnIndex, LedgerMintIndex},
    state::event::{DepositId, WithdrawSolRequest},
};
use candid::Principal;
use cksol_types::{DepositStatus, WithdrawSolStatus};
use cksol_types_internal::{Ed25519KeyName, InitArgs, UpgradeArgs};
use ic_canister_runtime::Runtime;
use ic_ed25519::PublicKey;
use icrc_ledger_types::icrc1::account::Account;
use sol_rpc_client::SolRpcClient;
use sol_rpc_types::{ConsensusStrategy, Lamport, RpcSources, SolanaCluster};
use solana_signature::Signature;
use std::cmp::Ordering;
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
};

#[cfg(test)]
mod tests;

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
    withdrawal_fee: Lamport,
    minimum_withdrawal_amount: Lamport,
    minimum_deposit_amount: Lamport,
    update_balance_required_cycles: u128,
    pending_update_balance_requests: BTreeSet<Account>,
    pending_withdraw_sol_requests: BTreeSet<Account>,
    accepted_deposits: BTreeMap<DepositId, Lamport>,
    quarantined_deposits: BTreeMap<DepositId, Lamport>,
    minted_deposits: BTreeMap<DepositId, MintedDeposit>,
    pending_withdrawal_requests: BTreeMap<LedgerBurnIndex, WithdrawSolRequest>,
    // TODO DEFI-2687: Sort by amount
    funds_to_consolidate: BTreeMap<Account, Lamport>,
    submitted_consolidation_requests: BTreeMap<Signature, Vec<(Account, Lamport)>>,
    available_balance: Lamport,
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

    pub fn withdrawal_fee(&self) -> u64 {
        self.withdrawal_fee
    }

    pub fn minimum_withdrawal_amount(&self) -> u64 {
        self.minimum_withdrawal_amount
    }

    pub fn minimum_deposit_amount(&self) -> u64 {
        self.minimum_deposit_amount
    }

    pub fn update_balance_required_cycles(&self) -> u128 {
        self.update_balance_required_cycles
    }

    pub fn deposit_status(&self, deposit_id: &DepositId) -> Option<DepositStatus> {
        if self.quarantined_deposits.contains_key(deposit_id) {
            return Some(DepositStatus::Quarantined(deposit_id.signature.into()));
        }
        if self.accepted_deposits.contains_key(deposit_id) {
            return Some(DepositStatus::Processing(deposit_id.signature.into()));
        }
        if let Some(MintedDeposit {
            block_index,
            minted_amount,
        }) = self.minted_deposits.get(deposit_id)
        {
            return Some(DepositStatus::Minted {
                block_index: *block_index.get(),
                minted_amount: *minted_amount,
                signature: deposit_id.signature.into(),
            });
        }
        None
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

    pub fn pending_withdraw_sol_requests_mut(&mut self) -> &mut BTreeSet<Account> {
        &mut self.pending_withdraw_sol_requests
    }

    fn validate(&self) -> Result<(), InvalidStateError> {
        let canister_ids: BTreeSet<_> = [self.sol_rpc_canister_id, self.ledger_canister_id]
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
        if self.minimum_deposit_amount < self.deposit_fee {
            return Err(InvalidStateError::InvalidMinimumDepositAmount {
                minimum_deposit_amount: self.minimum_deposit_amount,
                deposit_fee: self.deposit_fee,
            });
        }
        if self.minimum_withdrawal_amount < self.withdrawal_fee {
            return Err(InvalidStateError::InvalidMinimumWithdrawalAmount {
                minimum_withdrawal_amount: self.minimum_withdrawal_amount,
                withdrawal_fee: self.withdrawal_fee,
            });
        }
        Ok(())
    }

    fn upgrade(
        &mut self,
        UpgradeArgs {
            sol_rpc_canister_id,
            deposit_fee,
            minimum_withdrawal_amount,
            minimum_deposit_amount,
            withdrawal_fee,
            update_balance_required_cycles,
        }: UpgradeArgs,
    ) -> Result<(), InvalidStateError> {
        if let Some(sol_rpc_canister_id) = sol_rpc_canister_id {
            self.sol_rpc_canister_id = sol_rpc_canister_id;
        }
        if let Some(deposit_fee) = deposit_fee {
            self.deposit_fee = deposit_fee;
        }
        if let Some(withdrawal_fee) = withdrawal_fee {
            self.withdrawal_fee = withdrawal_fee;
        }
        if let Some(minimum_withdrawal_amount) = minimum_withdrawal_amount {
            self.minimum_withdrawal_amount = minimum_withdrawal_amount;
        }
        if let Some(minimum_deposit_amount) = minimum_deposit_amount {
            self.minimum_deposit_amount = minimum_deposit_amount;
        }
        if let Some(update_balance_required_cycles) = update_balance_required_cycles {
            self.update_balance_required_cycles = update_balance_required_cycles as u128;
        }
        self.validate()
    }

    fn process_accepted_deposit(
        &mut self,
        deposit_id: &DepositId,
        deposit_amount: &Lamport,
        amount_to_mint: &Lamport,
    ) {
        assert!(
            !self.quarantined_deposits.contains_key(deposit_id),
            "Attempted to accept already quarantined deposit: {deposit_id:?}"
        );
        assert!(
            !self.minted_deposits.contains_key(deposit_id),
            "Attempted to accept an already minted deposit: {deposit_id:?}"
        );
        assert!(
            self.accepted_deposits
                .insert(*deposit_id, *amount_to_mint)
                .is_none(),
            "Attempted to accept an already accepted deposit: {deposit_id:?}"
        );
        *self
            .funds_to_consolidate
            .entry(deposit_id.account)
            .or_default() += deposit_amount;
    }

    fn process_quarantined_deposit(&mut self, deposit_id: &DepositId) {
        assert!(
            !self.minted_deposits.contains_key(deposit_id),
            "Attempted to quarantine an already minted deposit: {deposit_id:?}"
        );
        let amount_to_mint = self
            .accepted_deposits
            .remove(deposit_id)
            .unwrap_or_else(|| {
                panic!("Attempted to quarantine an unknown deposit: {deposit_id:?}")
            });
        assert!(
            self.quarantined_deposits
                .insert(*deposit_id, amount_to_mint)
                .is_none(),
            "Attempted to quarantine already quarantined deposit: {deposit_id:?}"
        );
    }

    pub fn withdrawal_status(&self, block_index: u64) -> WithdrawSolStatus {
        if self
            .pending_withdrawal_requests
            .contains_key(&LedgerBurnIndex::from(block_index))
        {
            return WithdrawSolStatus::Pending;
        }
        WithdrawSolStatus::NotFound
    }

    fn process_accepted_withdrawal(&mut self, request: &WithdrawSolRequest) {
        assert_eq!(
            self.pending_withdrawal_requests
                .insert(request.burn_block_index, request.clone()),
            None,
            "Attempted to accept an already accepted withdrawal request: {:?}",
            request.burn_block_index
        );
    }

    fn process_mint(&mut self, deposit_id: &DepositId, mint_block_index: &LedgerMintIndex) {
        assert!(
            !self.quarantined_deposits.contains_key(deposit_id),
            "Attempted to mint ckSOL for a quarantined deposit: {deposit_id:?}",
        );
        let amount_to_mint = self
            .accepted_deposits
            .remove(deposit_id)
            .unwrap_or_else(|| {
                panic!("Attempted to mint ckSOL for an unknown deposit: {deposit_id:?}")
            });
        assert!(
            self.minted_deposits
                .insert(
                    *deposit_id,
                    MintedDeposit {
                        block_index: *mint_block_index,
                        minted_amount: amount_to_mint,
                    }
                )
                .is_none(),
            "Attempted to mint ckSOL twice for the same deposit: {deposit_id:?}",
        );
    }

    fn process_consolidation_request_submitted(
        &mut self,
        signature: &Signature,
        funds: &Vec<(Account, Lamport)>,
    ) {
        for (account, amount) in funds {
            let balance = self
                .funds_to_consolidate
                .get_mut(account)
                .unwrap_or_else(|| {
                    panic!("Attempted to consolidate funds for empty account: {account}")
                });
            match amount.cmp(balance) {
                Ordering::Greater => panic!(
                    "Attempted to consolidate {amount} lamports from account {account}, but deposit address only contains {balance} lamports"
                ),
                Ordering::Equal => {
                    self.funds_to_consolidate.remove(account);
                }
                Ordering::Less => *balance -= amount,
            }
        }
        assert!(
            self.submitted_consolidation_requests
                .insert(*signature, funds.clone())
                .is_none(),
            "Attempted to add consolidation transaction {signature:?} to submitted transactions twice"
        );
    }

    fn process_failed_transaction(&mut self, signature: &Signature) {
        if let Some(consolidated_funds) = self.submitted_consolidation_requests.remove(signature) {
            for (account, amount) in consolidated_funds {
                *self.funds_to_consolidate.entry(account).or_default() += amount;
            }
            return;
        }
        panic!("Processed unknown failed transaction {signature:?}");
    }

    fn process_finalized_transaction(&mut self, signature: &Signature) {
        if let Some(consolidated_funds) = self.submitted_consolidation_requests.remove(signature) {
            self.available_balance += consolidated_funds
                .into_iter()
                .map(|(_account, amount)| amount)
                .sum::<Lamport>();
            return;
        }
        panic!("Processed unknown finalized transaction {signature:?}");
    }
}

#[derive(Debug)]
pub enum InvalidStateError {
    InvalidCanisterId(String),
    InvalidMinimumDepositAmount {
        minimum_deposit_amount: u64,
        deposit_fee: u64,
    },
    InvalidMinimumWithdrawalAmount {
        minimum_withdrawal_amount: u64,
        withdrawal_fee: u64,
    },
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
            minimum_deposit_amount,
            withdrawal_fee,
            update_balance_required_cycles,
        }: InitArgs,
    ) -> Result<Self, Self::Error> {
        let state = Self {
            minter_public_key: None,
            master_key_name,
            ledger_canister_id,
            sol_rpc_canister_id,
            deposit_fee,
            withdrawal_fee,
            minimum_withdrawal_amount,
            minimum_deposit_amount,
            update_balance_required_cycles: update_balance_required_cycles as u128,
            pending_update_balance_requests: BTreeSet::new(),
            pending_withdraw_sol_requests: BTreeSet::new(),
            accepted_deposits: BTreeMap::new(),
            quarantined_deposits: BTreeMap::new(),
            minted_deposits: BTreeMap::new(),
            pending_withdrawal_requests: BTreeMap::new(),
            funds_to_consolidate: BTreeMap::new(),
            submitted_consolidation_requests: BTreeMap::new(),
            available_balance: 0,
        };
        state.validate()?;
        Ok(state)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SchnorrPublicKey {
    pub public_key: PublicKey,
    pub chain_code: [u8; 32],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MintedDeposit {
    block_index: LedgerMintIndex,
    minted_amount: Lamport,
}
