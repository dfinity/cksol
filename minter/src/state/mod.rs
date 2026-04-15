use crate::{
    constants::FEE_PER_SIGNATURE,
    ledger::client::LedgerClient,
    numeric::{LedgerBurnIndex, LedgerMintIndex},
    state::event::{DepositId, TransactionPurpose, VersionedMessage, WithdrawalRequest},
    utils::insertion_ordered_map::InsertionOrderedMap,
};
use candid::Principal;
use cksol_types::{DepositStatus, SolTransaction, TxFinalizedStatus, WithdrawalStatus};
use cksol_types_internal::SolanaNetwork;
use cksol_types_internal::{Ed25519KeyName, InitArgs, UpgradeArgs};
use ic_canister_runtime::Runtime;
use ic_ed25519::PublicKey;
use icrc_ledger_types::icrc1::account::Account;
use sol_rpc_client::SolRpcClient;
use sol_rpc_types::{ConsensusStrategy, Lamport, RpcSources, Slot, SolanaCluster};
use solana_signature::Signature;
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
};

#[cfg(test)]
mod tests;

pub mod audit;
pub mod event;

/// The minimum balance required for a Solana account to be rent-exempt.
/// This is the rent-exemption threshold for a basic account with 0 data bytes.
pub const SOLANA_RENT_EXEMPTION_THRESHOLD: Lamport = 890_880;

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

#[cfg(any(test, feature = "canbench-rs"))]
pub fn reset_state() {
    STATE.with(|s| {
        *s.borrow_mut() = None;
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
    solana_network: SolanaNetwork,
    deposit_fee: Lamport,
    withdrawal_fee: Lamport,
    minimum_withdrawal_amount: Lamport,
    minimum_deposit_amount: Lamport,
    update_balance_required_cycles: u128,
    deposit_consolidation_fee: u128,
    pending_update_balance_requests: BTreeSet<Account>,
    pending_withdrawal_request_guards: BTreeSet<Account>,
    accepted_deposits: InsertionOrderedMap<DepositId, Deposit>,
    quarantined_deposits: InsertionOrderedMap<DepositId, Deposit>,
    minted_deposits: InsertionOrderedMap<DepositId, MintedDeposit>,
    pending_withdrawal_requests: BTreeMap<LedgerBurnIndex, PendingWithdrawalRequest>,
    sent_withdrawal_requests: BTreeMap<LedgerBurnIndex, SentWithdrawalRequest>,
    successful_withdrawal_requests: BTreeMap<LedgerBurnIndex, SentWithdrawalRequest>,
    failed_withdrawal_requests: BTreeMap<LedgerBurnIndex, SentWithdrawalRequest>,
    deposits_to_consolidate: BTreeMap<LedgerMintIndex, (Account, Lamport)>,
    submitted_transactions: InsertionOrderedMap<Signature, SolanaTransaction>,
    transactions_to_resubmit: InsertionOrderedMap<Signature, SolanaTransaction>,
    succeeded_transactions: BTreeSet<Signature>,
    failed_transactions: InsertionOrderedMap<Signature, SolanaTransaction>,
    consolidation_transactions: InsertionOrderedMap<Signature, ConsolidationTransaction>,
    active_tasks: BTreeSet<TaskType>,
    balance: Lamport,
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

    pub fn deposit_consolidation_fee(&self) -> u128 {
        self.deposit_consolidation_fee
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

    pub fn solana_network(&self) -> SolanaNetwork {
        self.solana_network
    }

    pub fn update_balance_required_cycles(&self) -> u128 {
        self.update_balance_required_cycles
    }

    pub fn accepted_deposits(&self) -> &InsertionOrderedMap<DepositId, Deposit> {
        &self.accepted_deposits
    }

    pub fn quarantined_deposits(&self) -> &InsertionOrderedMap<DepositId, Deposit> {
        &self.quarantined_deposits
    }

    pub fn minted_deposits(&self) -> &InsertionOrderedMap<DepositId, MintedDeposit> {
        &self.minted_deposits
    }

    pub fn sent_withdrawal_requests(&self) -> &BTreeMap<LedgerBurnIndex, SentWithdrawalRequest> {
        &self.sent_withdrawal_requests
    }

    pub fn successful_withdrawal_requests(
        &self,
    ) -> &BTreeMap<LedgerBurnIndex, SentWithdrawalRequest> {
        &self.successful_withdrawal_requests
    }

    pub fn failed_withdrawal_requests(&self) -> &BTreeMap<LedgerBurnIndex, SentWithdrawalRequest> {
        &self.failed_withdrawal_requests
    }

    pub fn deposits_to_consolidate(&self) -> &BTreeMap<LedgerMintIndex, (Account, Lamport)> {
        &self.deposits_to_consolidate
    }

    pub fn submitted_transactions(&self) -> &InsertionOrderedMap<Signature, SolanaTransaction> {
        &self.submitted_transactions
    }

    pub fn transactions_to_resubmit(&self) -> &InsertionOrderedMap<Signature, SolanaTransaction> {
        &self.transactions_to_resubmit
    }

    pub fn process_transaction_expired(&mut self, signature: &Signature) {
        assert!(
            !self.succeeded_transactions.contains(signature),
            "BUG: cannot mark already succeeded transaction {signature} for resubmission"
        );
        assert!(
            !self.failed_transactions.contains_key(signature),
            "BUG: cannot mark already failed transaction {signature} for resubmission"
        );
        let transaction = self
            .submitted_transactions
            .remove(signature)
            .unwrap_or_else(|| {
                panic!("BUG: cannot mark non-submitted transaction {signature} for resubmission")
            });
        assert!(
            self.transactions_to_resubmit
                .insert(*signature, transaction)
                .is_none(),
            "BUG: transaction {signature} is already queued for resubmission"
        );
    }

    pub fn succeeded_transactions(&self) -> &BTreeSet<Signature> {
        &self.succeeded_transactions
    }

    pub fn failed_transactions(&self) -> &InsertionOrderedMap<Signature, SolanaTransaction> {
        &self.failed_transactions
    }

    pub fn balance(&self) -> Lamport {
        self.balance
    }

    pub fn consolidation_transactions(
        &self,
    ) -> &InsertionOrderedMap<Signature, ConsolidationTransaction> {
        &self.consolidation_transactions
    }

    pub fn deposit_status(&self, deposit_id: &DepositId) -> Option<DepositStatus> {
        if self.quarantined_deposits.contains_key(deposit_id) {
            return Some(DepositStatus::Quarantined((*deposit_id).into()));
        }
        if let Some(Deposit {
            deposit_amount,
            amount_to_mint,
        }) = self.accepted_deposits.get(deposit_id)
        {
            return Some(DepositStatus::Processing {
                deposit_amount: *deposit_amount,
                amount_to_mint: *amount_to_mint,
                deposit_id: (*deposit_id).into(),
            });
        }
        if let Some(MintedDeposit {
            block_index,
            deposit: Deposit { amount_to_mint, .. },
        }) = self.minted_deposits.get(deposit_id)
        {
            return Some(DepositStatus::Minted {
                block_index: *block_index.get(),
                minted_amount: *amount_to_mint,
                deposit_id: (*deposit_id).into(),
            });
        }
        None
    }

    pub fn sol_rpc_client<R: Runtime>(&self, runtime: R) -> SolRpcClient<R> {
        SolRpcClient::builder(runtime, self.sol_rpc_canister_id)
            .with_rpc_sources(RpcSources::Default(SolanaCluster::from(
                self.solana_network,
            )))
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

    pub fn pending_withdrawal_request_guards_mut(&mut self) -> &mut BTreeSet<Account> {
        &mut self.pending_withdrawal_request_guards
    }

    pub fn active_tasks_mut(&mut self) -> &mut BTreeSet<TaskType> {
        &mut self.active_tasks
    }

    fn transaction_fee(&self, message: &VersionedMessage) -> Lamport {
        let VersionedMessage::Legacy(msg) = message;
        FEE_PER_SIGNATURE * msg.header.num_required_signatures as u64
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
        let minimum_required = self.withdrawal_fee + SOLANA_RENT_EXEMPTION_THRESHOLD;
        if self.minimum_withdrawal_amount < minimum_required {
            return Err(InvalidStateError::InvalidMinimumWithdrawalAmount {
                minimum_withdrawal_amount: self.minimum_withdrawal_amount,
                withdrawal_fee: self.withdrawal_fee,
                rent_exemption_threshold: SOLANA_RENT_EXEMPTION_THRESHOLD,
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
            deposit_consolidation_fee,
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
        if let Some(deposit_consolidation_fee) = deposit_consolidation_fee {
            self.deposit_consolidation_fee = deposit_consolidation_fee as u128;
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
        assert_eq!(
            self.accepted_deposits.insert(
                *deposit_id,
                Deposit {
                    deposit_amount: *deposit_amount,
                    amount_to_mint: *amount_to_mint,
                }
            ),
            None,
            "Attempted to accept an already accepted deposit: {deposit_id:?}"
        );
    }

    fn process_quarantined_deposit(&mut self, deposit_id: &DepositId) {
        assert!(
            !self.minted_deposits.contains_key(deposit_id),
            "Attempted to quarantine an already minted deposit: {deposit_id:?}"
        );
        let accepted_deposit = self
            .accepted_deposits
            .remove(deposit_id)
            .unwrap_or_else(|| {
                panic!("Attempted to quarantine an unknown deposit: {deposit_id:?}")
            });
        assert_eq!(
            self.quarantined_deposits
                .insert(*deposit_id, accepted_deposit),
            None,
            "Attempted to quarantine already quarantined deposit: {deposit_id:?}"
        );
    }

    pub fn withdrawal_status(&self, block_index: u64) -> WithdrawalStatus {
        let burn_index = LedgerBurnIndex::from(block_index);
        if self.pending_withdrawal_requests.contains_key(&burn_index) {
            return WithdrawalStatus::Pending;
        }
        if let Some(sent) = self.sent_withdrawal_requests.get(&burn_index) {
            return WithdrawalStatus::TxSent(SolTransaction {
                transaction_hash: sent.signature.to_string(),
            });
        }
        if let Some(sent) = self.successful_withdrawal_requests.get(&burn_index) {
            return WithdrawalStatus::TxFinalized(TxFinalizedStatus::Success {
                transaction_hash: sent.signature.to_string(),
                effective_transaction_fee: None,
            });
        }
        if let Some(sent) = self.failed_withdrawal_requests.get(&burn_index) {
            return WithdrawalStatus::TxFinalized(TxFinalizedStatus::Failure {
                transaction_hash: sent.signature.to_string(),
            });
        }
        WithdrawalStatus::NotFound
    }

    pub fn pending_withdrawal_requests(
        &self,
    ) -> &BTreeMap<LedgerBurnIndex, PendingWithdrawalRequest> {
        &self.pending_withdrawal_requests
    }

    /// Returns the creation timestamp (in nanoseconds) of the oldest incomplete withdrawal request.
    /// An incomplete withdrawal is one that has not yet been finalized (succeeded or failed).
    pub fn oldest_incomplete_withdrawal_created_at(&self) -> Option<u64> {
        let pending = self
            .pending_withdrawal_requests
            .values()
            .map(|r| r.created_at);
        let sent = self.sent_withdrawal_requests.values().map(|r| r.created_at);
        pending.chain(sent).min()
    }

    fn process_accepted_withdrawal(&mut self, request: &WithdrawalRequest, created_at: u64) {
        assert_eq!(
            self.pending_withdrawal_requests.insert(
                request.burn_block_index,
                PendingWithdrawalRequest {
                    request: request.clone(),
                    created_at,
                }
            ),
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
        let deposit = self
            .accepted_deposits
            .remove(deposit_id)
            .unwrap_or_else(|| {
                panic!("Attempted to mint ckSOL for an unknown deposit: {deposit_id:?}")
            });
        assert_eq!(
            self.deposits_to_consolidate.insert(
                *mint_block_index,
                (deposit_id.account, deposit.deposit_amount)
            ),
            None,
            "Attempted to consolidate funds for an already consolidated mint index: {mint_block_index:?}",
        );
        assert_eq!(
            self.minted_deposits.insert(
                *deposit_id,
                MintedDeposit {
                    block_index: *mint_block_index,
                    deposit,
                }
            ),
            None,
            "Attempted to mint ckSOL twice for the same deposit: {deposit_id:?}",
        );
    }

    fn process_transaction_submitted(
        &mut self,
        signature: &Signature,
        transaction: &VersionedMessage,
        signers: &[Account],
        slot: Slot,
        purpose: &TransactionPurpose,
    ) {
        assert!(
            !self.succeeded_transactions.contains(signature),
            "Attempted to submit already succeeded transaction {signature:?}"
        );
        assert!(
            !self.failed_transactions.contains_key(signature),
            "Attempted to submit already failed transaction {signature:?}"
        );
        let amount = match purpose {
            TransactionPurpose::ConsolidateDeposits { mint_indices } => {
                let mut total: Lamport = 0;
                let mut deposits = Vec::with_capacity(mint_indices.len());
                for mint_index in mint_indices {
                    let (_account, deposit_amount) = self
                        .deposits_to_consolidate
                        .remove(mint_index)
                        .unwrap_or_else(|| {
                            panic!("Attempted to consolidate unknown mint index: {mint_index:?}")
                        });
                    total += deposit_amount;
                    deposits.push((*mint_index, deposit_amount));
                }
                self.consolidation_transactions
                    .insert(*signature, ConsolidationTransaction { deposits });
                total
            }
            TransactionPurpose::WithdrawSol { burn_indices } => {
                let mut total: Lamport = 0;
                for burn_index in burn_indices {
                    let pending = self
                        .pending_withdrawal_requests
                        .remove(burn_index)
                        .unwrap_or_else(|| {
                            panic!("Attempted to send transaction for unknown withdrawal request: {burn_index:?}")
                        });
                    total += pending.request.withdrawal_amount;
                    assert_eq!(
                        self.sent_withdrawal_requests.insert(
                            *burn_index,
                            SentWithdrawalRequest {
                                request: pending.request,
                                signature: *signature,
                                created_at: pending.created_at,
                            },
                        ),
                        None,
                        "Attempted to send transaction for already sent withdrawal request: {burn_index:?}"
                    );
                }
                let tx_fee = self.transaction_fee(transaction);
                self.balance = self
                    .balance
                    .checked_sub(total + tx_fee)
                    .expect("BUG: insufficient minter balance for withdrawal");
                total
            }
        };
        assert_eq!(
            self.submitted_transactions.insert(
                *signature,
                SolanaTransaction {
                    message: transaction.clone(),
                    signers: signers.to_vec(),
                    slot,
                    purpose: purpose.clone(),
                    amount,
                }
            ),
            None,
            "Attempted to submit transaction with signature {signature:?} twice"
        );
    }

    fn process_transaction_resubmitted(
        &mut self,
        old_signature: &Signature,
        new_signature: &Signature,
        new_slot: Slot,
    ) {
        let old_transaction = self
            .transactions_to_resubmit
            .remove(old_signature)
            .unwrap_or_else(|| {
                panic!("Attempted to resubmit unknown transaction with signature {old_signature:?}")
            });
        assert!(
            !self.succeeded_transactions.contains(new_signature),
            "Attempted to resubmit with signature {new_signature:?} that already succeeded"
        );
        assert!(
            !self.failed_transactions.contains_key(new_signature),
            "Attempted to resubmit with signature {new_signature:?} that already failed"
        );
        let new_transaction = SolanaTransaction {
            slot: new_slot,
            ..old_transaction
        };
        assert_eq!(
            self.submitted_transactions
                .insert(*new_signature, new_transaction),
            None,
            "Attempted to resubmit transaction with signature {new_signature:?} that already exists"
        );
        if let Some(info) = self.consolidation_transactions.remove(old_signature) {
            self.consolidation_transactions.insert(*new_signature, info);
        }
        for sent in self.sent_withdrawal_requests.values_mut() {
            if &sent.signature == old_signature {
                sent.signature = *new_signature;
            }
        }
    }

    fn process_transaction_succeeded(&mut self, signature: &Signature) {
        assert!(
            !self.failed_transactions.contains_key(signature),
            "Attempted to mark already failed transaction {signature:?} as succeeded"
        );
        let transaction = self
            .submitted_transactions
            .remove(signature)
            .unwrap_or_else(|| {
                panic!("Attempted to mark unknown transaction {signature:?} as succeeded")
            });
        if matches!(
            transaction.purpose,
            TransactionPurpose::ConsolidateDeposits { .. }
        ) {
            let tx_fee = self.transaction_fee(&transaction.message);
            self.balance += transaction
                .amount
                .checked_sub(tx_fee)
                .expect("BUG: consolidation amount is less than transaction fee");
        }
        assert!(
            !self.transactions_to_resubmit.contains_key(signature),
            "BUG: transaction {signature} is queued for resubmission but is being marked as succeeded"
        );
        assert!(
            self.succeeded_transactions.insert(*signature),
            "Attempted to mark transaction {signature:?} as succeeded twice"
        );
        self.sent_withdrawal_requests
            .extract_if(.., |_, sent| &sent.signature == signature)
            .for_each(|(burn_index, sent)| {
                self.successful_withdrawal_requests.insert(burn_index, sent);
            });
    }

    fn process_transaction_failed(&mut self, signature: &Signature) {
        assert!(
            !self.succeeded_transactions.contains(signature),
            "Attempted to mark already succeeded transaction {signature:?} as failed"
        );
        let transaction = self
            .submitted_transactions
            .remove(signature)
            .unwrap_or_else(|| {
                panic!("Attempted to mark unknown transaction {signature:?} as failed")
            });
        assert!(
            !self.transactions_to_resubmit.contains_key(signature),
            "BUG: transaction {signature} is queued for resubmission but is being marked as failed"
        );
        assert_eq!(
            self.failed_transactions.insert(*signature, transaction),
            None,
            "Attempted to fail transaction {signature:?} twice"
        );
        self.sent_withdrawal_requests
            .extract_if(.., |_, sent| &sent.signature == signature)
            .for_each(|(burn_index, sent)| {
                self.failed_withdrawal_requests.insert(burn_index, sent);
            });
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum InvalidStateError {
    InvalidCanisterId(String),
    InvalidMinimumDepositAmount {
        minimum_deposit_amount: u64,
        deposit_fee: u64,
    },
    InvalidMinimumWithdrawalAmount {
        minimum_withdrawal_amount: u64,
        withdrawal_fee: u64,
        rent_exemption_threshold: u64,
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
            solana_network,
            deposit_consolidation_fee,
        }: InitArgs,
    ) -> Result<Self, Self::Error> {
        let state = Self {
            minter_public_key: None,
            master_key_name,
            ledger_canister_id,
            sol_rpc_canister_id,
            solana_network,
            deposit_fee,
            withdrawal_fee,
            minimum_withdrawal_amount,
            minimum_deposit_amount,
            update_balance_required_cycles: update_balance_required_cycles as u128,
            deposit_consolidation_fee: deposit_consolidation_fee as u128,
            pending_update_balance_requests: BTreeSet::new(),
            pending_withdrawal_request_guards: BTreeSet::new(),
            accepted_deposits: InsertionOrderedMap::new(),
            quarantined_deposits: InsertionOrderedMap::new(),
            minted_deposits: InsertionOrderedMap::new(),
            pending_withdrawal_requests: BTreeMap::new(),
            sent_withdrawal_requests: BTreeMap::new(),
            successful_withdrawal_requests: BTreeMap::new(),
            failed_withdrawal_requests: BTreeMap::new(),
            deposits_to_consolidate: BTreeMap::new(),
            submitted_transactions: InsertionOrderedMap::new(),
            transactions_to_resubmit: InsertionOrderedMap::new(),
            succeeded_transactions: BTreeSet::new(),
            failed_transactions: InsertionOrderedMap::new(),
            consolidation_transactions: InsertionOrderedMap::new(),
            active_tasks: BTreeSet::new(),
            balance: 0,
        };
        state.validate()?;
        Ok(state)
    }
}

/// A pending withdrawal request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingWithdrawalRequest {
    pub request: WithdrawalRequest,
    pub created_at: u64,
}

/// A withdrawal request that has been submitted in a Solana transaction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SentWithdrawalRequest {
    pub request: WithdrawalRequest,
    pub signature: Signature,
    pub created_at: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SchnorrPublicKey {
    pub public_key: PublicKey,
    pub chain_code: [u8; 32],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Deposit {
    pub deposit_amount: Lamport,
    pub amount_to_mint: Lamport,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MintedDeposit {
    pub block_index: LedgerMintIndex,
    pub deposit: Deposit,
}

#[derive(Copy, Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum TaskType {
    DepositConsolidation,
    Mint,
    FinalizeTransactions,
    ResubmitTransactions,
    WithdrawalProcessing,
}

/// Details about a consolidation transaction, capturing the individual
/// deposits (by mint index and amount) being consolidated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConsolidationTransaction {
    pub deposits: Vec<(LedgerMintIndex, Lamport)>,
}

impl ConsolidationTransaction {
    pub fn total_amount(&self) -> Lamport {
        self.deposits.iter().map(|(_, amount)| amount).sum()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SolanaTransaction {
    pub message: VersionedMessage,
    pub signers: Vec<Account>,
    pub slot: Slot,
    pub purpose: TransactionPurpose,
    /// Total transfer amount in lamports (excluding fees).
    pub amount: Lamport,
}
