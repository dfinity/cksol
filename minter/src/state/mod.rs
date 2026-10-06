use crate::{
    address::{account_address, minter_address},
    constants::{FEE_PER_SIGNATURE, GET_BALANCE_CYCLES, RENT_EXEMPTION_THRESHOLD},
    ledger::client::LedgerClient,
    numeric::{LedgerBurnIndex, LedgerMintIndex},
    rpc::BlockHeight,
    sol_transfer::{BATCH_WITHDRAWAL_TX_FEE, MAX_SIGNATURES, MAX_WITHDRAWALS_PER_TX},
    state::event::{
        CreditedDeposit, Signer, TransactionPurpose, VersionedMessage, WithdrawalRequest,
    },
    utils::insertion_ordered_map::InsertionOrderedMap,
};
use candid::Principal;
use cksol_types::{DepositSolId, TxFinalizedStatus, WithdrawalStatus};
use cksol_types_internal::SolanaNetwork;
use cksol_types_internal::{Ed25519KeyName, InitArgs, UpgradeArgs};
use ic_canister_runtime::Runtime;
use ic_ed25519::PublicKey;
use icrc_ledger_types::icrc1::account::Account;
use sol_rpc_client::SolRpcClient;
use sol_rpc_types::{ConsensusStrategy, Lamport, RpcSources, SolanaCluster};
use solana_address::Address;
use solana_hash::Hash;
use solana_signature::Signature;
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet, btree_map},
    iter::Peekable,
    str::FromStr,
};

#[cfg(test)]
mod tests;

pub mod audit;
mod deposits;
pub mod event;
mod nonce_pool;

pub use deposits::{
    DepositBalance, Deposits, MintedSweep, PendingMint, QuarantineCause, QuarantinedDeposit,
    QueuedDeposit, SettledSweep, Sweep, SweepMismatch, SweepRecoveryError, SweepSettlementError,
    Sweeps, SweptDeposit, Transfer, UnreadableOutcome,
};
pub use nonce_pool::{DurableNoncePool, NoncePoolError};

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
    withdrawal_fee: Lamport,
    minimum_withdrawal_amount: Lamport,
    minimum_deposit_amount: Lamport,
    deposit_sol_required_cycles: u128,
    deposit_sol_fee: u128,
    pending_deposit_sol_request_guards: BTreeSet<Account>,
    pending_withdrawal_request_guards: BTreeSet<Account>,
    deposits: Deposits,
    pending_withdrawal_requests: BTreeMap<LedgerBurnIndex, PendingWithdrawalRequest>,
    created_withdrawal_requests: BTreeMap<LedgerBurnIndex, PendingWithdrawalRequest>,
    sent_withdrawal_requests: BTreeMap<LedgerBurnIndex, SentWithdrawalRequest>,
    successful_withdrawal_requests: BTreeMap<LedgerBurnIndex, SentWithdrawalRequest>,
    failed_withdrawal_requests: BTreeMap<LedgerBurnIndex, SentWithdrawalRequest>,
    submitted_transactions: InsertionOrderedMap<Signature, MinterTransaction>,
    created_withdrawal_txs: BTreeMap<Address, CreatedWithdrawalTransaction>,
    transactions_to_resubmit: InsertionOrderedMap<Signature, MinterTransaction>,
    succeeded_transactions: BTreeSet<Signature>,
    failed_transactions: InsertionOrderedMap<Signature, MinterTransaction>,
    nonce_pool: DurableNoncePool,
    active_tasks: BTreeSet<TaskType>,
    balance: Lamport,
}

impl State {
    pub fn minter_public_key(&self) -> Option<&SchnorrPublicKey> {
        self.minter_public_key.as_ref()
    }

    /// Cache the minter public key.
    ///
    /// Concurrent calls may each fetch the key before either one caches it.
    /// All of them fetch with identical arguments and thus obtain the same key,
    /// so caching the same key again is a no-op.
    ///
    /// # Panics
    /// This method will panic if a different public key is already cached,
    /// since the minter public key must never change.
    pub fn cache_minter_public_key(&mut self, public_key: SchnorrPublicKey) {
        match &self.minter_public_key {
            None => self.minter_public_key = Some(public_key),
            Some(cached) if *cached == public_key => {}
            Some(_) => panic!("BUG: attempt to overwrite the minter public key"),
        }
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

    pub fn deposit_sol_fee(&self) -> u128 {
        self.deposit_sol_fee
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

    pub fn deposit_sol_required_cycles(&self) -> u128 {
        self.deposit_sol_required_cycles
    }

    pub fn deposits(&self) -> &Deposits {
        &self.deposits
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

    pub fn submitted_transactions(&self) -> &InsertionOrderedMap<Signature, MinterTransaction> {
        &self.submitted_transactions
    }

    pub fn created_withdrawal_txs(&self) -> &BTreeMap<Address, CreatedWithdrawalTransaction> {
        &self.created_withdrawal_txs
    }

    pub fn transactions_to_resubmit(&self) -> &InsertionOrderedMap<Signature, MinterTransaction> {
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
        match transaction {
            MinterTransaction::SweepDeposit { .. } => self.deposits.drop_swept(signature),
            MinterTransaction::Withdrawal { .. } => assert!(
                self.transactions_to_resubmit
                    .insert(*signature, transaction)
                    .is_none(),
                "BUG: transaction {signature} is already queued for resubmission"
            ),
            MinterTransaction::NonceWithdrawal { .. } => {
                panic!("BUG: durable-nonce withdrawal transaction {signature} cannot expire")
            }
        }
    }

    pub fn succeeded_transactions(&self) -> &BTreeSet<Signature> {
        &self.succeeded_transactions
    }

    pub fn failed_transactions(&self) -> &InsertionOrderedMap<Signature, MinterTransaction> {
        &self.failed_transactions
    }

    pub fn balance(&self) -> Lamport {
        self.balance
    }

    pub fn nonce_pool(&self) -> &DurableNoncePool {
        &self.nonce_pool
    }

    pub fn nonce_pool_addresses(&self) -> BTreeSet<Address> {
        self.nonce_pool.addresses().copied().collect()
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

    pub fn pending_deposit_sol_request_guards_mut(&mut self) -> &mut BTreeSet<Account> {
        &mut self.pending_deposit_sol_request_guards
    }

    pub fn pending_withdrawal_request_guards_mut(&mut self) -> &mut BTreeSet<Account> {
        &mut self.pending_withdrawal_request_guards
    }

    pub fn active_tasks_mut(&mut self) -> &mut BTreeSet<TaskType> {
        &mut self.active_tasks
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
        let maximum_sweep_fee = MAX_SIGNATURES * FEE_PER_SIGNATURE;
        if self.minimum_deposit_amount < maximum_sweep_fee + RENT_EXEMPTION_THRESHOLD {
            return Err(InvalidStateError::InvalidMinimumDepositAmount {
                minimum_deposit_amount: self.minimum_deposit_amount,
                maximum_sweep_fee,
                rent_exemption_threshold: RENT_EXEMPTION_THRESHOLD,
            });
        }
        if self.minimum_deposit_amount < 2 * RENT_EXEMPTION_THRESHOLD + FEE_PER_SIGNATURE {
            return Err(
                InvalidStateError::MinimumDepositAmountLeavesMainAddressBelowRent {
                    minimum_deposit_amount: self.minimum_deposit_amount,
                    rent_exemption_threshold: RENT_EXEMPTION_THRESHOLD,
                    fee_per_signature: FEE_PER_SIGNATURE,
                },
            );
        }
        if self.minimum_withdrawal_amount < self.withdrawal_fee + RENT_EXEMPTION_THRESHOLD {
            return Err(InvalidStateError::InvalidMinimumWithdrawalAmount {
                minimum_withdrawal_amount: self.minimum_withdrawal_amount,
                withdrawal_fee: self.withdrawal_fee,
                rent_exemption_threshold: RENT_EXEMPTION_THRESHOLD,
            });
        }
        if self.deposit_sol_required_cycles < GET_BALANCE_CYCLES + self.deposit_sol_fee {
            return Err(InvalidStateError::DepositSolRequiredCyclesTooLow {
                required_cycles: self.deposit_sol_required_cycles,
                get_balance_cycles: GET_BALANCE_CYCLES,
                deposit_sol_fee: self.deposit_sol_fee,
            });
        }
        Ok(())
    }

    fn upgrade(
        &mut self,
        UpgradeArgs {
            sol_rpc_canister_id,
            minimum_withdrawal_amount,
            minimum_deposit_amount,
            withdrawal_fee,
            deposit_sol_required_cycles,
            deposit_sol_fee,
            nonce_accounts_to_add,
        }: UpgradeArgs,
    ) -> Result<(), InvalidStateError> {
        if let Some(sol_rpc_canister_id) = sol_rpc_canister_id {
            self.sol_rpc_canister_id = sol_rpc_canister_id;
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
        if let Some(deposit_sol_required_cycles) = deposit_sol_required_cycles {
            self.deposit_sol_required_cycles = deposit_sol_required_cycles as u128;
        }
        if let Some(deposit_sol_fee) = deposit_sol_fee {
            self.deposit_sol_fee = deposit_sol_fee as u128;
        }
        if let Some(nonce_accounts) = nonce_accounts_to_add {
            let nonce_accounts = parse_nonce_accounts(nonce_accounts)?;
            self.ensure_no_incomplete_withdrawal_to(&nonce_accounts)?;
            self.nonce_pool.add_accounts(nonce_accounts)?;
        }
        self.validate()
    }

    /// An incomplete withdrawal survives an upgrade, so an address may only
    /// join the nonce pool once no queued or in-flight transfer targets it;
    /// otherwise the withdrawal would credit a minter-controlled account.
    fn ensure_no_incomplete_withdrawal_to(
        &self,
        nonce_accounts: &[Address],
    ) -> Result<(), InvalidStateError> {
        let incomplete_destinations: BTreeSet<Address> = self
            .pending_withdrawal_requests
            .values()
            .chain(self.created_withdrawal_requests.values())
            .map(|pending| &pending.request)
            .chain(
                self.sent_withdrawal_requests
                    .values()
                    .map(|sent| &sent.request),
            )
            .map(|request| Address::from(request.solana_address))
            .collect();
        match nonce_accounts
            .iter()
            .find(|address| incomplete_destinations.contains(address))
        {
            Some(address) => Err(InvalidStateError::NonceAccountIsWithdrawalDestination(
                *address,
            )),
            None => Ok(()),
        }
    }

    fn process_queued_deposit(
        &mut self,
        deposit_id: DepositSolId,
        account: &Account,
        address: &Address,
        balance: DepositBalance,
    ) {
        debug_assert_eq!(
            *address,
            account_address(
                self.minter_public_key
                    .as_ref()
                    .expect("BUG: a deposit was queued before the minter public key was recorded"),
                account,
            ),
            "Attempted to queue deposit {deposit_id} with address {address} not derived from account {account:?}",
        );
        self.deposits.queue(
            deposit_id,
            QueuedDeposit {
                account: *account,
                address: *address,
                balance,
            },
        );
    }

    fn process_credited_sweep(
        &mut self,
        signature: &Signature,
        amount_received: Lamport,
        mints: &[CreditedDeposit],
        timestamp: u64,
    ) {
        let amount_to_mint: Lamport = mints.iter().map(|mint| mint.amount_to_mint).sum();
        assert!(
            amount_to_mint <= amount_received,
            "Attempted to credit sweep {signature} with mints of {amount_to_mint} lamports exceeding the {amount_received} lamports received"
        );
        self.deposits.credit_sweep(signature, mints, timestamp);
        self.balance += amount_received;
    }

    fn process_minted_swept_deposit(
        &mut self,
        deposit_id: DepositSolId,
        mint_block_index: &LedgerMintIndex,
    ) {
        self.deposits.mint(deposit_id, *mint_block_index);
    }

    fn process_quarantined_pending_mint(&mut self, deposit_id: DepositSolId) {
        self.deposits.quarantine_pending_mint(deposit_id);
    }

    fn process_quarantined_sweep(&mut self, signature: &Signature) {
        self.deposits.quarantine_sweep(signature);
    }

    pub fn withdrawal_status(&self, block_index: u64) -> WithdrawalStatus {
        let burn_index = LedgerBurnIndex::from(block_index);
        if self.pending_withdrawal_requests.contains_key(&burn_index)
            || self.created_withdrawal_requests.contains_key(&burn_index)
        {
            return WithdrawalStatus::Pending;
        }
        if let Some(sent) = self.sent_withdrawal_requests.get(&burn_index) {
            return WithdrawalStatus::TxSent {
                transaction_id: sent.signature.into(),
            };
        }
        if let Some(sent) = self.successful_withdrawal_requests.get(&burn_index) {
            return WithdrawalStatus::TxFinalized(TxFinalizedStatus::Success {
                transaction_id: sent.signature.into(),
                effective_transaction_fee: None,
            });
        }
        if let Some(sent) = self.failed_withdrawal_requests.get(&burn_index) {
            return WithdrawalStatus::TxFinalized(TxFinalizedStatus::Failure {
                transaction_id: sent.signature.into(),
            });
        }
        WithdrawalStatus::NotFound
    }

    pub fn pending_withdrawal_requests(
        &self,
    ) -> &BTreeMap<LedgerBurnIndex, PendingWithdrawalRequest> {
        &self.pending_withdrawal_requests
    }

    pub fn created_withdrawal_requests(
        &self,
    ) -> &BTreeMap<LedgerBurnIndex, PendingWithdrawalRequest> {
        &self.created_withdrawal_requests
    }

    pub fn withdrawal_batches(&self) -> WithdrawalBatches<'_> {
        WithdrawalBatches {
            pending_requests: self.pending_withdrawal_requests.values().peekable(),
            available_balance: self.balance,
        }
    }

    /// Returns the creation timestamp (in nanoseconds) of the oldest incomplete withdrawal request.
    /// An incomplete withdrawal is one that has not yet been finalized (succeeded or failed).
    pub fn oldest_incomplete_withdrawal_created_at(&self) -> Option<u64> {
        let pending = self
            .pending_withdrawal_requests
            .values()
            .map(|r| r.created_at);
        let created = self
            .created_withdrawal_requests
            .values()
            .map(|r| r.created_at);
        let sent = self.sent_withdrawal_requests.values().map(|r| r.created_at);
        pending.chain(created).chain(sent).min()
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

    fn process_transaction_submitted(
        &mut self,
        signature: &Signature,
        transaction: &VersionedMessage,
        signers: &[Signer],
        purpose: &TransactionPurpose,
        block_height: BlockHeight,
    ) {
        assert!(
            !self.succeeded_transactions.contains(signature),
            "Attempted to submit already succeeded transaction {signature:?}"
        );
        assert!(
            !self.failed_transactions.contains_key(signature),
            "Attempted to submit already failed transaction {signature:?}"
        );
        let message = transaction.clone();
        let signers = signers.to_vec();
        let submitted_transaction = match purpose {
            TransactionPurpose::WithdrawSol { burn_indices } => {
                let mut total: Lamport = 0;
                for burn_index in burn_indices {
                    let pending = self
                        .pending_withdrawal_requests
                        .remove(burn_index)
                        .unwrap_or_else(|| {
                            panic!("Attempted to send transaction for unknown withdrawal request: {burn_index:?}")
                        });
                    total += pending.request.amount_to_transfer;
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
                let tx_fee = transaction.transaction_fee();
                self.balance = self
                    .balance
                    .checked_sub(total + tx_fee)
                    .expect("BUG: insufficient minter balance for withdrawal");
                MinterTransaction::Withdrawal {
                    message,
                    signers,
                    block_height,
                }
            }
            TransactionPurpose::SweepDeposits { deposit_ids } => {
                let sweep_destination = minter_address(self.minter_public_key.as_ref().expect(
                    "BUG: a sweep was submitted before the minter public key was recorded",
                ));
                self.deposits
                    .sweep(deposit_ids, sweep_destination, transaction, signature);
                MinterTransaction::SweepDeposit {
                    message,
                    signers,
                    block_height,
                }
            }
        };
        assert_eq!(
            self.submitted_transactions
                .insert(*signature, submitted_transaction),
            None,
            "Attempted to submit transaction with signature {signature:?} twice"
        );
    }

    fn process_transaction_created(
        &mut self,
        message: &VersionedMessage,
        burn_indices: &[LedgerBurnIndex],
        nonce_account: &Address,
        nonce_value: Hash,
    ) {
        self.nonce_pool.bind(nonce_account, nonce_value);
        let mut total: Lamport = 0;
        for burn_index in burn_indices {
            let pending = self
                .pending_withdrawal_requests
                .remove(burn_index)
                .unwrap_or_else(|| {
                    panic!(
                        "Attempted to create transaction for unknown withdrawal request: {burn_index:?}"
                    )
                });
            total += pending.request.amount_to_transfer;
            assert_eq!(
                self.created_withdrawal_requests
                    .insert(*burn_index, pending),
                None,
                "Attempted to create transaction for already created withdrawal request: {burn_index:?}"
            );
        }
        self.balance = self
            .balance
            .checked_sub(total + message.transaction_fee())
            .expect("BUG: insufficient minter balance for withdrawal");
        assert_eq!(
            self.created_withdrawal_txs.insert(
                *nonce_account,
                CreatedWithdrawalTransaction {
                    message: message.clone(),
                    nonce_value,
                    burn_indices: burn_indices.to_vec(),
                }
            ),
            None,
            "BUG: nonce account {nonce_account} already has a created transaction"
        );
    }

    fn process_transaction_signed(&mut self, signature: &Signature, nonce_account: &Address) {
        let created = self
            .created_withdrawal_txs
            .remove(nonce_account)
            .unwrap_or_else(|| {
                panic!("Attempted to sign unknown created transaction for nonce account {nonce_account}")
            });
        for burn_index in &created.burn_indices {
            let pending = self
                .created_withdrawal_requests
                .remove(burn_index)
                .unwrap_or_else(|| {
                    panic!(
                        "BUG: withdrawal request {burn_index:?} of a created transaction is not in the created bucket"
                    )
                });
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
        assert_eq!(
            self.submitted_transactions.insert(
                *signature,
                MinterTransaction::NonceWithdrawal {
                    message: created.message,
                    signers: vec![Signer::Minter],
                    nonce_account: *nonce_account,
                    nonce_value: created.nonce_value,
                }
            ),
            None,
            "Attempted to sign withdrawal transaction {signature} twice"
        );
    }

    fn process_transaction_resubmitted(
        &mut self,
        old_signature: &Signature,
        new_signature: &Signature,
        new_block_height: BlockHeight,
    ) {
        let old_transaction = self
            .transactions_to_resubmit
            .remove(old_signature)
            .unwrap_or_else(|| {
                panic!("Attempted to resubmit unknown transaction with signature {old_signature:?}")
            });
        let new_transaction = match old_transaction {
            MinterTransaction::SweepDeposit { .. } => panic!(
                "BUG: sweep transaction {old_signature} must be dropped instead of resubmitted"
            ),
            MinterTransaction::Withdrawal {
                message,
                signers,
                block_height: _,
            } => MinterTransaction::Withdrawal {
                message,
                signers,
                block_height: new_block_height,
            },
            MinterTransaction::NonceWithdrawal { .. } => panic!(
                "BUG: durable-nonce withdrawal transaction {old_signature} must never be resubmitted"
            ),
        };
        assert!(
            !self.succeeded_transactions.contains(new_signature),
            "Attempted to resubmit with signature {new_signature:?} that already succeeded"
        );
        assert!(
            !self.failed_transactions.contains_key(new_signature),
            "Attempted to resubmit with signature {new_signature:?} that already failed"
        );
        assert_eq!(
            self.submitted_transactions
                .insert(*new_signature, new_transaction),
            None,
            "Attempted to resubmit transaction with signature {new_signature:?} that already exists"
        );
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
        match transaction {
            MinterTransaction::SweepDeposit { .. } => self.deposits.finalize_swept(signature),
            MinterTransaction::Withdrawal { .. } => {}
            MinterTransaction::NonceWithdrawal { nonce_account, .. } => {
                self.nonce_pool.free(&nonce_account)
            }
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
        match &transaction {
            MinterTransaction::SweepDeposit { .. } => self.deposits.drop_swept(signature),
            MinterTransaction::Withdrawal { .. } => {}
            MinterTransaction::NonceWithdrawal { nonce_account, .. } => {
                self.nonce_pool.free(nonce_account)
            }
        }
        assert_eq!(
            self.failed_transactions.insert(*signature, transaction),
            None,
            "Attempted to fail transaction {signature:?} twice"
        );
        assert!(
            !self.transactions_to_resubmit.contains_key(signature),
            "BUG: transaction {signature} is queued for resubmission but is being marked as failed"
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
        maximum_sweep_fee: u64,
        rent_exemption_threshold: u64,
    },
    MinimumDepositAmountLeavesMainAddressBelowRent {
        minimum_deposit_amount: u64,
        rent_exemption_threshold: u64,
        fee_per_signature: u64,
    },
    InvalidMinimumWithdrawalAmount {
        minimum_withdrawal_amount: u64,
        withdrawal_fee: u64,
        rent_exemption_threshold: u64,
    },
    DepositSolRequiredCyclesTooLow {
        required_cycles: u128,
        get_balance_cycles: u128,
        deposit_sol_fee: u128,
    },
    InvalidNonceAccount(String),
    DuplicateNonceAccount(Address),
    NonceAccountIsWithdrawalDestination(Address),
}

impl From<NoncePoolError> for InvalidStateError {
    fn from(error: NoncePoolError) -> Self {
        match error {
            NoncePoolError::DuplicateAccount(address) => Self::DuplicateNonceAccount(address),
        }
    }
}

fn parse_nonce_accounts(addresses: Vec<String>) -> Result<Vec<Address>, InvalidStateError> {
    addresses
        .into_iter()
        .map(|address| {
            Address::from_str(&address).map_err(|error| {
                InvalidStateError::InvalidNonceAccount(format!(
                    "ERROR: failed to parse nonce account {address}: {error}"
                ))
            })
        })
        .collect()
}

impl TryFrom<InitArgs> for State {
    type Error = InvalidStateError;

    fn try_from(
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
        }: InitArgs,
    ) -> Result<Self, Self::Error> {
        let nonce_pool = DurableNoncePool::new(parse_nonce_accounts(nonce_accounts)?)?;
        let state = Self {
            minter_public_key: None,
            master_key_name,
            ledger_canister_id,
            sol_rpc_canister_id,
            solana_network,
            withdrawal_fee,
            minimum_withdrawal_amount,
            minimum_deposit_amount,
            deposit_sol_required_cycles: deposit_sol_required_cycles as u128,
            deposit_sol_fee: deposit_sol_fee as u128,
            pending_deposit_sol_request_guards: BTreeSet::new(),
            pending_withdrawal_request_guards: BTreeSet::new(),
            deposits: Deposits::default(),
            pending_withdrawal_requests: BTreeMap::new(),
            created_withdrawal_requests: BTreeMap::new(),
            sent_withdrawal_requests: BTreeMap::new(),
            successful_withdrawal_requests: BTreeMap::new(),
            failed_withdrawal_requests: BTreeMap::new(),
            submitted_transactions: InsertionOrderedMap::new(),
            created_withdrawal_txs: BTreeMap::new(),
            transactions_to_resubmit: InsertionOrderedMap::new(),
            succeeded_transactions: BTreeSet::new(),
            failed_transactions: InsertionOrderedMap::new(),
            nonce_pool,
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

/// Groups pending withdrawal requests, oldest first, into batches that the
/// minter balance can pay for, including one transaction fee per batch.
///
/// Iteration stops at the first request the remaining balance cannot cover,
/// so requests are never reordered or skipped.
pub struct WithdrawalBatches<'a> {
    pending_requests: Peekable<btree_map::Values<'a, LedgerBurnIndex, PendingWithdrawalRequest>>,
    available_balance: Lamport,
}

impl Iterator for WithdrawalBatches<'_> {
    type Item = Vec<WithdrawalRequest>;

    fn next(&mut self) -> Option<Self::Item> {
        let mut batch = Vec::new();
        while batch.len() < MAX_WITHDRAWALS_PER_TX {
            let reserved_fee = if batch.is_empty() {
                BATCH_WITHDRAWAL_TX_FEE
            } else {
                0
            };
            let Some(pending) = self.pending_requests.peek() else {
                break;
            };
            let Some(remaining_balance) = pending
                .request
                .amount_to_transfer
                .checked_add(reserved_fee)
                .and_then(|cost| self.available_balance.checked_sub(cost))
            else {
                break;
            };
            self.available_balance = remaining_balance;
            batch.push(pending.request.clone());
            self.pending_requests.next();
        }
        if batch.is_empty() { None } else { Some(batch) }
    }
}

/// A withdrawal transaction recorded before its threshold signature was
/// requested, so that a signing failure leads to re-signing the identical
/// message instead of building a new one for the same nonce value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CreatedWithdrawalTransaction {
    pub message: VersionedMessage,
    pub nonce_value: Hash,
    pub burn_indices: Vec<LedgerBurnIndex>,
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

#[derive(Copy, Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum TaskType {
    SweepDeposits,
    Mint,
    FinalizeTransactions,
    ResubmitTransactions,
    WithdrawalProcessing,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MinterTransaction {
    SweepDeposit {
        message: VersionedMessage,
        signers: Vec<Signer>,
        /// The block height of the block whose blockhash the transaction uses.
        block_height: BlockHeight,
    },
    Withdrawal {
        message: VersionedMessage,
        signers: Vec<Signer>,
        /// The block height of the block whose blockhash the transaction uses.
        block_height: BlockHeight,
    },
    /// A durable-nonce withdrawal transaction, which never expires: it stays
    /// in flight until it is finalized.
    NonceWithdrawal {
        message: VersionedMessage,
        signers: Vec<Signer>,
        /// The durable nonce account whose nonce value the transaction uses.
        nonce_account: Address,
        /// The durable nonce value the transaction uses instead of a recent blockhash.
        nonce_value: Hash,
    },
}

impl MinterTransaction {
    pub fn message(&self) -> &VersionedMessage {
        match self {
            MinterTransaction::SweepDeposit { message, .. }
            | MinterTransaction::Withdrawal { message, .. }
            | MinterTransaction::NonceWithdrawal { message, .. } => message,
        }
    }

    pub fn signers(&self) -> &[Signer] {
        match self {
            MinterTransaction::SweepDeposit { signers, .. }
            | MinterTransaction::Withdrawal { signers, .. }
            | MinterTransaction::NonceWithdrawal { signers, .. } => signers,
        }
    }

    /// The block height of the block whose blockhash the transaction uses,
    /// or `None` for a durable-nonce transaction, which never expires.
    pub fn block_height(&self) -> Option<BlockHeight> {
        match self {
            MinterTransaction::SweepDeposit { block_height, .. }
            | MinterTransaction::Withdrawal { block_height, .. } => Some(*block_height),
            MinterTransaction::NonceWithdrawal { .. } => None,
        }
    }
}
