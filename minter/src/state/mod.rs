use crate::{
    address::{account_address, minter_address},
    constants::{FEE_PER_SIGNATURE, GET_BALANCE_CYCLES, RENT_EXEMPTION_THRESHOLD},
    ledger::client::LedgerClient,
    numeric::{LedgerBurnIndex, LedgerMintIndex},
    rpc::BlockHeight,
    sol_transfer::{
        BATCH_WITHDRAWAL_TX_FEE, CreateTransferError, MAX_SIGNATURES, MAX_WITHDRAWALS_PER_NONCE_TX,
        build_batch_withdrawal_message,
    },
    spl_transfer::{build_spl_withdrawal_message, withdrawal::SplWithdrawalTransfer},
    state::event::{
        CreditedDeposit, Signer, SplWithdrawalRequest, SplWithdrawalRequestTerms,
        TransactionPurpose, VersionedMessage, WatchlistSnapshot, WithdrawalRequest,
    },
    utils::insertion_ordered_map::InsertionOrderedMap,
};
use candid::Principal;
use cksol_types::{
    DepositSolId, DepositSplId, SplRefundKind, SplRentRefundStatus, TxFinalizedStatus,
    WithdrawSolStatus, WithdrawSplId, WithdrawSplStatus,
};
use cksol_types_internal::SolanaNetwork;
use cksol_types_internal::{Ed25519KeyName, InitArgs, UpgradeArgs};
use ic_canister_runtime::Runtime;
use ic_ed25519::PublicKey;
use icrc_ledger_types::icrc1::account::Account;
use sol_rpc_client::{ClientBuilder, SolRpcClient};
use sol_rpc_types::{ConsensusStrategy, Lamport, RpcSources, SolanaCluster};
use solana_address::Address;
use solana_hash::Hash;
use solana_message::Message;
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
pub mod operating_balance;
mod spl;
mod spl_deposits;
pub(crate) mod sweep;
mod watchlist;

pub use deposits::{
    DepositBalance, DepositOrigin, Deposits, InFlightDeposit, MintedSweep, PendingMint,
    QuarantineCause, QuarantinedDeposit, QueuedDeposit, SettledSweep, Sweep, SweepMismatch,
    SweepRecoveryError, SweepSettlementError, Sweeps, SweptDeposit, Transfer, UnreadableOutcome,
};
pub use nonce_pool::{DurableNoncePool, NoncePoolError, NonceRead};
pub use spl::SupportedSplToken;
pub use spl_deposits::{
    InFlightSplDeposit, MintedSplSweep, PendingSplMint, QuarantinedSplDeposit, QueuedSplDeposit,
    SPL_SWEEP_TRANSACTION_FEE, SettledSplSweep, SplDeposits, SplQuarantineCause, SplSweep,
    SplSweepRecoveryError, SplSweeps, SubmittedSplSweep, SweptSplDeposit,
};
pub use watchlist::{
    MAX_WATCHED_ADDRESSES, WATCH_WINDOW, WatchedAddress, Watchlist, WatchlistFull,
};

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
    crate::deposit::sweep::reset_cursors();
    crate::deposit::spl::reset_cursor();
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
#[derive(Clone, Debug, PartialEq, Eq)]
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
    automatic_deposit_fee: Option<Lamport>,
    collected_automatic_deposit_fees: Lamport,
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
    succeeded_transactions: BTreeSet<Signature>,
    failed_transactions: InsertionOrderedMap<Signature, MinterTransaction>,
    nonce_pool: DurableNoncePool,
    active_tasks: BTreeSet<TaskType>,
    balance: Lamport,
    supported_spl_tokens: BTreeMap<Address, spl::SupportedSplToken>,
    operating_balance: operating_balance::OperatingBalance,
    spl_deposits: SplDeposits,
    spl_deposit_consolidation_fee: u128,
    spl_withdrawal_fee: Lamport,
    spl_treasury_balances: BTreeMap<Address, u64>,
    pending_spl_withdrawal_requests: BTreeMap<LedgerBurnIndex, PendingSplWithdrawalRequest>,
    created_spl_withdrawal_requests: BTreeMap<LedgerBurnIndex, PendingSplWithdrawalRequest>,
    created_spl_withdrawal_txs: BTreeMap<Address, CreatedSplWithdrawalTransaction>,
    sent_spl_withdrawal_requests: BTreeMap<LedgerBurnIndex, SentSplWithdrawalRequest>,
    successful_spl_withdrawal_requests: BTreeMap<LedgerBurnIndex, SuccessfulSplWithdrawalRequest>,
    failed_spl_withdrawal_requests: BTreeMap<LedgerBurnIndex, SentSplWithdrawalRequest>,
    pending_spl_withdrawal_reimbursements:
        BTreeMap<WithdrawSplId, PendingSplWithdrawalReimbursement>,
    completed_spl_withdrawal_reimbursements:
        BTreeMap<WithdrawSplId, CompletedSplWithdrawalReimbursement>,
    quarantined_spl_withdrawal_reimbursements:
        BTreeMap<WithdrawSplId, PendingSplWithdrawalReimbursement>,
    collected_spl_withdrawal_fees: BTreeMap<Address, u128>,
    spl_withdrawal_token_burns: BTreeSet<(Principal, LedgerBurnIndex)>,
    rejected_spl_withdrawal_requests: BTreeMap<LedgerBurnIndex, RejectedSplWithdrawalRequest>,
    pending_spl_cksol_refunds: BTreeMap<WithdrawSplId, PendingSplCkSolRefund>,
    completed_spl_cksol_refunds: BTreeMap<WithdrawSplId, CompletedSplCkSolRefund>,
    quarantined_spl_cksol_refunds: BTreeMap<WithdrawSplId, PendingSplCkSolRefund>,
    pending_deposit_spl_request_guards: BTreeSet<(Account, Address)>,
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

    pub fn automatic_deposit_fee(&self) -> Option<Lamport> {
        self.automatic_deposit_fee
    }

    pub fn collected_automatic_deposit_fees(&self) -> Lamport {
        self.collected_automatic_deposit_fees
    }

    pub fn deposits(&self) -> &Deposits {
        &self.deposits
    }

    pub fn spl_deposits(&self) -> &SplDeposits {
        &self.spl_deposits
    }

    pub fn spl_deposit_consolidation_fee(&self) -> u128 {
        self.spl_deposit_consolidation_fee
    }

    /// The ckSOL fee of an SPL withdrawal, excluding the rent reserve of the destination.
    pub fn spl_withdrawal_fee(&self) -> Lamport {
        self.spl_withdrawal_fee
    }

    /// The ckSOL burned with the fee for the rent of the destination token account.
    pub fn spl_withdrawal_rent_reserve(&self) -> Lamport {
        crate::constants::CLASSIC_TOKEN_ACCOUNT_RENT_EXEMPTION
    }

    pub fn deposit_spl_required_cycles(&self) -> u128 {
        self.spl_deposit_consolidation_fee
            .checked_add(crate::constants::GET_ACCOUNT_INFO_CYCLES)
            .expect("BUG: SPL deposit cycle budget overflow")
    }

    pub fn pending_deposit_spl_request_guards_mut(&mut self) -> &mut BTreeSet<(Account, Address)> {
        &mut self.pending_deposit_spl_request_guards
    }

    pub fn watchlist(&self) -> &Watchlist {
        self.deposits.watchlist()
    }

    pub fn watchlist_mut(&mut self) -> &mut Watchlist {
        self.deposits.watchlist_mut()
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

    pub fn process_transaction_expired(&mut self, signature: &Signature) {
        assert!(
            !self.succeeded_transactions.contains(signature),
            "BUG: cannot mark already succeeded transaction {signature} as expired"
        );
        assert!(
            !self.failed_transactions.contains_key(signature),
            "BUG: cannot mark already failed transaction {signature} as expired"
        );
        let transaction = self
            .submitted_transactions
            .remove(signature)
            .unwrap_or_else(|| {
                panic!("BUG: cannot mark non-submitted transaction {signature} as expired")
            });
        match transaction {
            MinterTransaction::SweepDeposit { .. } => self.deposits.drop_swept(signature),
            MinterTransaction::SweepSplDeposit { .. } => {
                // A proven expiry spent nothing, so the entire reservation returns.
                let sweep = self.spl_deposits.drop_swept(signature);
                self.return_spl_sweep_cost(sweep.reserved_lamports);
            }
            MinterTransaction::Withdrawal { .. } | MinterTransaction::SplWithdrawal { .. } => {
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
        self.sol_rpc_client_builder(runtime).build()
    }

    pub fn sol_rpc_client_builder<R: Runtime>(&self, runtime: R) -> ClientBuilder<R> {
        SolRpcClient::builder(runtime, self.sol_rpc_canister_id)
            .with_rpc_sources(RpcSources::Default(SolanaCluster::from(
                self.solana_network,
            )))
            .with_consensus_strategy(ConsensusStrategy::Threshold {
                min: 3,
                total: Some(4),
            })
    }

    pub fn ledger_client<R: Runtime>(&self, runtime: R) -> LedgerClient<R> {
        LedgerClient::new(runtime, self.ledger_canister_id)
    }

    pub fn pending_deposit_sol_request_guards_mut(&mut self) -> &mut BTreeSet<Account> {
        &mut self.pending_deposit_sol_request_guards
    }

    /// Whether a `deposit_sol` call for the account is waiting for its balance read.
    pub fn is_deposit_sol_in_progress(&self, account: &Account) -> bool {
        self.pending_deposit_sol_request_guards.contains(account)
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
        if let Some(automatic_deposit_fee) = self.automatic_deposit_fee
            && automatic_deposit_fee > self.max_automatic_deposit_fee()
        {
            return Err(InvalidStateError::AutomaticDepositFeeTooHigh {
                automatic_deposit_fee,
                max_automatic_deposit_fee: self.max_automatic_deposit_fee(),
            });
        }
        // The minter pays the transaction fee of an SPL withdrawal from this fee.
        if self.spl_withdrawal_fee < BATCH_WITHDRAWAL_TX_FEE {
            return Err(InvalidStateError::SplWithdrawalFeeTooLow {
                spl_withdrawal_fee: self.spl_withdrawal_fee,
                transaction_fee: BATCH_WITHDRAWAL_TX_FEE,
            });
        }
        spl::validate_tokens(
            &self.supported_spl_tokens,
            self.ledger_canister_id,
            self.sol_rpc_canister_id,
        )?;
        self.operating_balance.validate()
    }

    fn max_automatic_deposit_fee(&self) -> Lamport {
        self.minimum_deposit_amount
            .saturating_sub(RENT_EXEMPTION_THRESHOLD)
            .saturating_sub(FEE_PER_SIGNATURE)
            .saturating_sub(1)
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
            automatic_deposit_fee,
            add_spl_tokens,
            update_spl_tokens,
            operating_balance_refill,
            spl_deposit_consolidation_fee,
            spl_withdrawal_fee,
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
        if let Some(automatic_deposit_fee) = automatic_deposit_fee {
            self.automatic_deposit_fee = Some(automatic_deposit_fee);
        }
        if let Some(nonce_accounts) = nonce_accounts_to_add {
            let nonce_accounts = parse_nonce_accounts(nonce_accounts)?;
            self.ensure_no_incomplete_withdrawal_to(&nonce_accounts)?;
            self.nonce_pool.add_accounts(nonce_accounts)?;
        }
        self.supported_spl_tokens = self.updated_spl_tokens(add_spl_tokens, update_spl_tokens)?;
        if let Some(config) = operating_balance_refill {
            self.operating_balance.set_config(config);
        }
        if let Some(fee) = spl_deposit_consolidation_fee {
            self.spl_deposit_consolidation_fee = u128::from(fee);
        }
        if let Some(fee) = spl_withdrawal_fee {
            self.spl_withdrawal_fee = fee;
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

    fn process_queued_spl_deposit(
        &mut self,
        deposit_id: cksol_types::DepositSplId,
        deposit: QueuedSplDeposit,
        queued_at: u64,
    ) {
        debug_assert_eq!(
            deposit.address,
            crate::address::associated_token_address(
                &account_address(
                    self.minter_public_key.as_ref().expect(
                        "BUG: an SPL deposit was queued before the minter public key was recorded"
                    ),
                    &deposit.account,
                ),
                &deposit.mint,
            ),
            "Attempted to queue SPL deposit {deposit_id} with an unexpected source ATA",
        );
        self.spl_deposits.queue(deposit_id, deposit, queued_at);
    }

    fn process_queued_deposit(
        &mut self,
        deposit_id: DepositSolId,
        deposit: QueuedDeposit,
        queued_at: u64,
    ) {
        debug_assert_eq!(
            deposit.address,
            account_address(
                self.minter_public_key
                    .as_ref()
                    .expect("BUG: a deposit was queued before the minter public key was recorded"),
                &deposit.account,
            ),
            "Attempted to queue deposit {deposit_id} with address {} not derived from account {:?}",
            deposit.address,
            deposit.account,
        );
        self.deposits.queue(deposit_id, deposit, queued_at);
    }

    fn restore_watchlist(&mut self, snapshot: &WatchlistSnapshot) {
        *self.deposits.watchlist_mut() = Watchlist::from_snapshot(snapshot);
    }

    fn process_credited_sweep(
        &mut self,
        signature: &Signature,
        amount_received: Lamport,
        mints: &[CreditedDeposit],
        timestamp: u64,
    ) {
        let amount_to_mint: Lamport = mints.iter().map(|mint| mint.amount_to_mint).sum();
        let sweep = self.deposits.credit_sweep(signature, mints, timestamp);
        let deposit_fees = sweep.deposit_fees();
        assert!(
            amount_to_mint
                .checked_add(deposit_fees)
                .is_some_and(|total| total <= amount_received),
            "Attempted to credit sweep {signature} with mints of {amount_to_mint} lamports and deposit fees of {deposit_fees} lamports exceeding the {amount_received} lamports received"
        );
        self.collected_automatic_deposit_fees += sweep.automatic_deposit_fees();
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

    /// Funds available for SPL fees, excluding ckSOL backing and the main account's rent.
    pub fn spl_sweep_allowance(&self) -> Lamport {
        let operating_sol = self.operating_balance.balance();
        operating_sol.min(self.spendable_balance())
    }

    /// The queued SPL deposits whose sweep the allowance does not cover.
    pub fn unfunded_queued_spl_deposits(&self) -> u64 {
        let funded_sweeps = self.spl_sweep_allowance() / SPL_SWEEP_TRANSACTION_FEE;
        (self.spl_deposits.queued().len() as u64).saturating_sub(funded_sweeps)
    }

    pub fn covers_spl_sweep(&self, sweep: &SplSweep) -> bool {
        self.spl_sweep_allowance() >= SPL_SWEEP_TRANSACTION_FEE
            && self.spl_deposits.queued().get(&sweep.deposit_id()) == Some(sweep.deposit())
    }

    fn sweep_spl_deposit(
        &mut self,
        signature: &Signature,
        deposit_id: DepositSplId,
        message: &VersionedMessage,
        signers: &[Signer],
    ) {
        assert!(
            !self.submitted_transactions.contains_key(signature),
            "Attempted to submit transaction with signature {signature:?} twice"
        );
        assert!(
            self.spl_sweep_allowance() >= SPL_SWEEP_TRANSACTION_FEE,
            "Attempted to submit an unfunded SPL sweep"
        );
        let master_key = self
            .minter_public_key
            .as_ref()
            .expect("BUG: an SPL sweep was submitted before the minter public key was recorded");
        self.spl_deposits.sweep(
            deposit_id,
            message,
            signers,
            &self.supported_spl_tokens,
            master_key,
            signature,
            SPL_SWEEP_TRANSACTION_FEE,
        );
        self.reserve_spl_sweep_cost();
    }

    /// Withholds the cost of an SPL sweep from both spendable balances until its outcome
    /// is settled.
    fn reserve_spl_sweep_cost(&mut self) {
        let reserved = SPL_SWEEP_TRANSACTION_FEE;
        self.balance = self
            .balance
            .checked_sub(reserved)
            .expect("BUG: insufficient minter balance for an SPL sweep");
        self.operating_balance.reserve(reserved);
    }

    /// Returns the given part of a reservation to both spendable balances.
    fn return_spl_sweep_cost(&mut self, returned: Lamport) {
        let balance = self
            .balance
            .checked_add(returned)
            .expect("BUG: main SOL balance overflow while releasing an SPL reservation");
        self.operating_balance.release(returned);
        self.balance = balance;
    }

    fn process_credited_spl_sweep(
        &mut self,
        signature: &Signature,
        lamports_spent: Lamport,
        deposit_id: DepositSplId,
        amount_to_mint: u64,
        timestamp: u64,
    ) {
        let sweep = self
            .spl_deposits
            .finalized()
            .get(signature)
            .unwrap_or_else(|| {
                panic!("Attempted to credit SPL sweep {signature} that is not finalized")
            });
        let unspent = unspent_spl_sweep_cost(sweep, lamports_spent);
        let deposit = sweep.sweep.deposit();
        let (mint, swept_tokens) = (deposit.mint, deposit.balance);
        self.spl_deposits
            .credit_sweep(signature, deposit_id, amount_to_mint, timestamp);
        self.return_spl_sweep_cost(unspent);
        // The treasury received the gross amount: the minted part and the deposit fee.
        let treasury = self.spl_treasury_balances.entry(mint).or_default();
        *treasury = treasury
            .checked_add(swept_tokens)
            .expect("BUG: SPL treasury balance overflow");
    }

    /// Settles the validated cost of an SPL sweep whose failure was observed before.
    fn process_failed_spl_sweep(&mut self, signature: &Signature, lamports_spent: Lamport) {
        let sweep = self
            .spl_deposits
            .failed()
            .get(signature)
            .unwrap_or_else(|| {
                panic!("Attempted to settle SPL sweep {signature} whose failure was not recorded")
            });
        let unspent = unspent_spl_sweep_cost(sweep, lamports_spent);
        self.spl_deposits.drop_failed(signature);
        self.return_spl_sweep_cost(unspent);
    }

    fn process_quarantined_spl_sweep(&mut self, signature: &Signature) {
        self.spl_deposits.quarantine_sweep(signature);
    }

    fn process_minted_swept_spl_deposit(
        &mut self,
        deposit_id: DepositSplId,
        mint_block_index: &LedgerMintIndex,
    ) {
        self.spl_deposits.mint(deposit_id, *mint_block_index);
    }

    fn process_quarantined_pending_spl_mint(&mut self, deposit_id: DepositSplId) {
        self.spl_deposits.quarantine_pending_mint(deposit_id);
    }

    pub fn withdrawal_status(&self, block_index: u64) -> WithdrawSolStatus {
        let burn_index = LedgerBurnIndex::from(block_index);
        if self.pending_withdrawal_requests.contains_key(&burn_index)
            || self.created_withdrawal_requests.contains_key(&burn_index)
        {
            return WithdrawSolStatus::Pending;
        }
        if let Some(sent) = self.sent_withdrawal_requests.get(&burn_index) {
            return WithdrawSolStatus::TxSent {
                transaction_id: sent.signature.into(),
            };
        }
        if let Some(sent) = self.successful_withdrawal_requests.get(&burn_index) {
            return WithdrawSolStatus::TxFinalized(TxFinalizedStatus::Success {
                transaction_id: sent.signature.into(),
            });
        }
        if let Some(sent) = self.failed_withdrawal_requests.get(&burn_index) {
            return WithdrawSolStatus::TxFinalized(TxFinalizedStatus::Failure {
                transaction_id: sent.signature.into(),
            });
        }
        WithdrawSolStatus::NotFound
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

    /// The balance less the rent exemption threshold that the main address must keep.
    pub fn spendable_balance(&self) -> Lamport {
        self.balance.saturating_sub(RENT_EXEMPTION_THRESHOLD)
    }

    pub fn withdrawal_plans(&self) -> WithdrawalPlans<'_> {
        WithdrawalPlans {
            pending_requests: self.pending_withdrawal_requests.values().peekable(),
            pending_spl_requests: self.pending_spl_withdrawal_requests.values().peekable(),
            available_balance: self.spendable_balance(),
            available_tokens: self.spl_treasury_balances.clone(),
            sol_blocked: false,
        }
    }

    /// The tokens of the mint that no created or sent withdrawal has reserved.
    pub fn spl_treasury_balance(&self, mint: &Address) -> u64 {
        self.spl_treasury_balances.get(mint).copied().unwrap_or(0)
    }

    /// Whether the SOL for the fee and the rent, and the treasury tokens, cover the request.
    pub fn covers_spl_withdrawal(&self, request: &SplWithdrawalRequest) -> bool {
        self.spendable_balance() >= spl_withdrawal_sol_cost(&request.terms)
            && self.spl_treasury_balance(&request.terms.mint)
                >= spl_withdrawal_token_cost(&request.terms)
    }

    pub fn covers_withdrawal_batch(&self, requests: &[WithdrawalRequest]) -> bool {
        let cost = requests
            .iter()
            .fold(BATCH_WITHDRAWAL_TX_FEE, |cost, request| {
                cost.saturating_add(request.amount_to_transfer)
            });
        self.spendable_balance() >= cost
    }

    pub fn can_create_withdrawal_transaction(&self) -> bool {
        self.nonce_pool.num_free_accounts() > 0 && self.withdrawal_plans().next().is_some()
    }

    pub fn has_unsigned_withdrawal_transaction(&self) -> bool {
        !self.created_withdrawal_txs.is_empty() || !self.created_spl_withdrawal_txs.is_empty()
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

    pub fn pending_spl_withdrawal_requests(
        &self,
    ) -> &BTreeMap<LedgerBurnIndex, PendingSplWithdrawalRequest> {
        &self.pending_spl_withdrawal_requests
    }

    pub fn spl_withdrawal_status(&self, cksol_burn_block_index: u64) -> WithdrawSplStatus {
        let burn_index = LedgerBurnIndex::from(cksol_burn_block_index);
        if self
            .pending_spl_withdrawal_requests
            .contains_key(&burn_index)
            || self
                .created_spl_withdrawal_requests
                .contains_key(&burn_index)
        {
            return WithdrawSplStatus::Pending;
        }
        if let Some(sent) = self.sent_spl_withdrawal_requests.get(&burn_index) {
            return WithdrawSplStatus::TxSent {
                transaction_id: sent.signature.into(),
            };
        }
        if let Some(successful) = self.successful_spl_withdrawal_requests.get(&burn_index) {
            return WithdrawSplStatus::TxFinalized {
                transaction: TxFinalizedStatus::Success {
                    transaction_id: successful.signature.into(),
                },
                rent_refund: self.spl_rent_refund_status(cksol_burn_block_index),
            };
        }
        if let Some(failed) = self.failed_spl_withdrawal_requests.get(&burn_index) {
            let transaction_id = failed.signature.into();
            let reimbursed_amount = failed.request.terms.amount;
            let rent_refund = self.spl_rent_refund_status(cksol_burn_block_index);
            if let Some(completed) = self
                .completed_spl_withdrawal_reimbursements
                .get(&cksol_burn_block_index)
            {
                return WithdrawSplStatus::Reimbursed {
                    transaction_id,
                    reimbursed_amount,
                    mint_block_index: *completed.mint_block_index.get(),
                    rent_refund,
                };
            }
            if self
                .quarantined_spl_withdrawal_reimbursements
                .contains_key(&cksol_burn_block_index)
            {
                return WithdrawSplStatus::ReimbursementQuarantined {
                    transaction_id,
                    reimbursed_amount,
                    rent_refund,
                };
            }
            return WithdrawSplStatus::ReimbursementPending {
                transaction_id,
                reimbursed_amount,
                rent_refund,
            };
        }
        if self
            .rejected_spl_withdrawal_requests
            .contains_key(&burn_index)
        {
            if let Some(pending) = self.pending_spl_cksol_refunds.get(&cksol_burn_block_index) {
                return WithdrawSplStatus::FeeReimbursementPending {
                    reimbursed_amount: pending.amount,
                };
            }
            if let Some(completed) = self
                .completed_spl_cksol_refunds
                .get(&cksol_burn_block_index)
            {
                return WithdrawSplStatus::FeeReimbursed {
                    reimbursed_amount: completed.refund.amount,
                    mint_block_index: *completed.mint_block_index.get(),
                };
            }
            if let Some(quarantined) = self
                .quarantined_spl_cksol_refunds
                .get(&cksol_burn_block_index)
            {
                return WithdrawSplStatus::FeeReimbursementQuarantined {
                    reimbursed_amount: quarantined.amount,
                };
            }
        }
        WithdrawSplStatus::NotFound
    }

    pub fn created_spl_withdrawal_requests(
        &self,
    ) -> &BTreeMap<LedgerBurnIndex, PendingSplWithdrawalRequest> {
        &self.created_spl_withdrawal_requests
    }

    pub fn created_spl_withdrawal_txs(
        &self,
    ) -> &BTreeMap<Address, CreatedSplWithdrawalTransaction> {
        &self.created_spl_withdrawal_txs
    }

    pub fn sent_spl_withdrawal_requests(
        &self,
    ) -> &BTreeMap<LedgerBurnIndex, SentSplWithdrawalRequest> {
        &self.sent_spl_withdrawal_requests
    }

    /// The progress of the ckSOL refund of the rent reserve of a settled withdrawal.
    fn spl_rent_refund_status(&self, cksol_burn_block_index: WithdrawSplId) -> SplRentRefundStatus {
        if let Some(pending) = self.pending_spl_cksol_refunds.get(&cksol_burn_block_index) {
            return SplRentRefundStatus::Pending {
                amount: pending.amount,
            };
        }
        if let Some(completed) = self
            .completed_spl_cksol_refunds
            .get(&cksol_burn_block_index)
        {
            return SplRentRefundStatus::Reimbursed {
                amount: completed.refund.amount,
                mint_block_index: *completed.mint_block_index.get(),
            };
        }
        if let Some(quarantined) = self
            .quarantined_spl_cksol_refunds
            .get(&cksol_burn_block_index)
        {
            return SplRentRefundStatus::Quarantined {
                amount: quarantined.amount,
            };
        }
        SplRentRefundStatus::NotDue
    }

    pub fn successful_spl_withdrawal_requests(
        &self,
    ) -> &BTreeMap<LedgerBurnIndex, SuccessfulSplWithdrawalRequest> {
        &self.successful_spl_withdrawal_requests
    }

    pub fn failed_spl_withdrawal_requests(
        &self,
    ) -> &BTreeMap<LedgerBurnIndex, SentSplWithdrawalRequest> {
        &self.failed_spl_withdrawal_requests
    }

    pub fn pending_spl_withdrawal_reimbursements(
        &self,
    ) -> &BTreeMap<WithdrawSplId, PendingSplWithdrawalReimbursement> {
        &self.pending_spl_withdrawal_reimbursements
    }

    pub fn completed_spl_withdrawal_reimbursements(
        &self,
    ) -> &BTreeMap<WithdrawSplId, CompletedSplWithdrawalReimbursement> {
        &self.completed_spl_withdrawal_reimbursements
    }

    pub fn quarantined_spl_withdrawal_reimbursements(
        &self,
    ) -> &BTreeMap<WithdrawSplId, PendingSplWithdrawalReimbursement> {
        &self.quarantined_spl_withdrawal_reimbursements
    }

    /// The token withdrawal fees that stayed in the treasury of the mint.
    pub fn collected_spl_withdrawal_fees(&self, mint: &Address) -> u128 {
        self.collected_spl_withdrawal_fees
            .get(mint)
            .copied()
            .unwrap_or(0)
    }

    /// The sent SPL withdrawal that the transaction with this signature serves.
    pub fn sent_spl_withdrawal_of(
        &self,
        signature: &Signature,
    ) -> Option<&SentSplWithdrawalRequest> {
        self.sent_spl_withdrawal_requests
            .values()
            .find(|sent| &sent.signature == signature)
    }

    fn take_sent_spl_withdrawal(
        &mut self,
        signature: &Signature,
    ) -> (LedgerBurnIndex, SentSplWithdrawalRequest, MinterTransaction) {
        let transaction = self
            .submitted_transactions
            .remove(signature)
            .unwrap_or_else(|| {
                panic!("Attempted to settle unknown SPL withdrawal transaction {signature}")
            });
        let MinterTransaction::SplWithdrawal { nonce_account, .. } = &transaction else {
            panic!("Attempted to settle transaction {signature} that is not an SPL withdrawal")
        };
        self.nonce_pool.free(nonce_account);
        let burn_index = self
            .sent_spl_withdrawal_requests
            .iter()
            .find_map(|(burn_index, sent)| (&sent.signature == signature).then_some(*burn_index))
            .unwrap_or_else(|| {
                panic!("BUG: SPL withdrawal transaction {signature} serves no sent request")
            });
        let sent = self
            .sent_spl_withdrawal_requests
            .remove(&burn_index)
            .expect("BUG: the sent SPL withdrawal was found above");
        (burn_index, sent, transaction)
    }

    /// Returns the part of the SOL reservation that the transaction did not spend. A
    /// rent above the reserve is taken from the balance.
    fn settle_spl_withdrawal_sol_cost(
        &mut self,
        terms: &SplWithdrawalRequestTerms,
        lamports_spent: Lamport,
    ) {
        let reserved = spl_withdrawal_sol_cost(terms);
        self.balance = self
            .balance
            .checked_add(reserved)
            .and_then(|balance| balance.checked_sub(lamports_spent))
            .expect("BUG: the minter balance does not cover the cost of an SPL withdrawal");
    }

    /// Books the result of the fee of a settled SPL withdrawal against the cost that the
    /// minter paid itself. The rest of the fee is unbacked SOL for the minter's own costs.
    /// A cost above the fee is taken from the operating balance, so that it does not
    /// silently reduce the SOL that backs ckSOL. A remainder is a shortfall of the
    /// operating balance, which later funding repairs first.
    fn settle_spl_withdrawal_fee(&mut self, fee: Lamport, own_cost: Lamport) {
        match fee.checked_sub(own_cost) {
            Some(surplus) => self.operating_balance.credit(surplus),
            None => self.operating_balance.debit(own_cost - fee),
        }
    }

    fn process_spl_withdrawal_succeeded(
        &mut self,
        signature: &Signature,
        lamports_spent: Lamport,
        rent_spent: Lamport,
    ) {
        assert!(
            !self.failed_transactions.contains_key(signature),
            "Attempted to mark already failed transaction {signature} as succeeded"
        );
        let transaction_fee = lamports_spent
            .checked_sub(rent_spent)
            .expect("Attempted to settle an SPL withdrawal with a rent above its total cost");
        let (burn_index, sent, _) = self.take_sent_spl_withdrawal(signature);
        let terms = &sent.request.terms;
        self.settle_spl_withdrawal_sol_cost(terms, lamports_spent);
        // The fee pays the transaction fee and any rent above the reserve.
        let own_cost = transaction_fee
            .checked_add(rent_spent.saturating_sub(terms.rent_reserve))
            .expect("BUG: SPL withdrawal cost overflows");
        self.settle_spl_withdrawal_fee(terms.fee, own_cost);
        let collected = self
            .collected_spl_withdrawal_fees(&terms.mint)
            .checked_add(u128::from(terms.token_fee))
            .expect("BUG: collected SPL withdrawal fees overflow");
        self.collected_spl_withdrawal_fees
            .insert(terms.mint, collected);
        assert!(
            self.succeeded_transactions.insert(*signature),
            "Attempted to mark transaction {signature} as succeeded twice"
        );
        self.successful_spl_withdrawal_requests.insert(
            burn_index,
            SuccessfulSplWithdrawalRequest {
                request: sent.request,
                signature: sent.signature,
                created_at: sent.created_at,
                lamports_spent,
                rent_spent,
            },
        );
    }

    fn process_spl_withdrawal_failed(
        &mut self,
        signature: &Signature,
        lamports_spent: Lamport,
        failed_at: u64,
    ) {
        assert!(
            !self.succeeded_transactions.contains(signature),
            "Attempted to mark already succeeded transaction {signature} as failed"
        );
        let (burn_index, sent, transaction) = self.take_sent_spl_withdrawal(signature);
        let terms = &sent.request.terms;
        self.settle_spl_withdrawal_sol_cost(terms, lamports_spent);
        // No token moved, so the reserved tokens return to the treasury.
        let treasury = self.spl_treasury_balances.entry(terms.mint).or_default();
        *treasury = treasury
            .checked_add(spl_withdrawal_token_cost(terms))
            .expect("BUG: SPL treasury balance overflow");
        // The fee is kept. It pays the fee of the failed transaction.
        self.settle_spl_withdrawal_fee(terms.fee, lamports_spent);
        assert_eq!(
            self.failed_transactions.insert(*signature, transaction),
            None,
            "Attempted to fail transaction {signature} twice"
        );
        self.pending_spl_withdrawal_reimbursements.insert(
            *burn_index.get(),
            PendingSplWithdrawalReimbursement {
                request: sent.request.clone(),
                created_at: failed_at,
            },
        );
        self.failed_spl_withdrawal_requests.insert(burn_index, sent);
    }

    fn process_reimbursed_spl_withdrawal(
        &mut self,
        burn_index: LedgerBurnIndex,
        mint_block_index: &LedgerMintIndex,
        reimbursed_at: u64,
    ) {
        let pending = self
            .pending_spl_withdrawal_reimbursements
            .remove(burn_index.get())
            .unwrap_or_else(|| {
                panic!("Attempted to complete the reimbursement of SPL withdrawal {burn_index} that has no pending request")
            });
        self.completed_spl_withdrawal_reimbursements.insert(
            *burn_index.get(),
            CompletedSplWithdrawalReimbursement {
                request: pending.request,
                mint_block_index: *mint_block_index,
                created_at: pending.created_at,
                reimbursed_at,
            },
        );
    }

    fn process_quarantined_spl_withdrawal_reimbursement(&mut self, burn_index: LedgerBurnIndex) {
        let pending = self
            .pending_spl_withdrawal_reimbursements
            .remove(burn_index.get())
            .unwrap_or_else(|| {
                panic!("Attempted to quarantine the reimbursement of SPL withdrawal {burn_index} that has no pending request")
            });
        self.quarantined_spl_withdrawal_reimbursements
            .insert(*burn_index.get(), pending);
    }

    pub fn pending_spl_cksol_refunds(&self) -> &BTreeMap<WithdrawSplId, PendingSplCkSolRefund> {
        &self.pending_spl_cksol_refunds
    }

    pub fn completed_spl_cksol_refunds(&self) -> &BTreeMap<WithdrawSplId, CompletedSplCkSolRefund> {
        &self.completed_spl_cksol_refunds
    }

    pub fn quarantined_spl_cksol_refunds(&self) -> &BTreeMap<WithdrawSplId, PendingSplCkSolRefund> {
        &self.quarantined_spl_cksol_refunds
    }

    fn has_spl_cksol_refund(&self, burn_index: &LedgerBurnIndex) -> bool {
        self.pending_spl_cksol_refunds
            .contains_key(burn_index.get())
            || self
                .completed_spl_cksol_refunds
                .contains_key(burn_index.get())
            || self
                .quarantined_spl_cksol_refunds
                .contains_key(burn_index.get())
    }

    fn process_queued_spl_cksol_refund(
        &mut self,
        burn_index: LedgerBurnIndex,
        kind: SplRefundKind,
        to: Account,
        amount: Lamport,
        created_at_time: u64,
    ) {
        assert!(
            !self.has_spl_cksol_refund(&burn_index),
            "Attempted to queue a second ckSOL refund for SPL withdrawal {burn_index}"
        );
        assert!(
            amount > 0,
            "Attempted to queue a ckSOL refund of zero for SPL withdrawal {burn_index}"
        );
        match kind {
            SplRefundKind::FailedTokenBurn => {
                let rejected = self
                    .rejected_spl_withdrawal_requests
                    .get(&burn_index)
                    .unwrap_or_else(|| {
                        panic!("Attempted to refund the fee of SPL withdrawal {burn_index} whose token burn did not fail")
                    });
                assert_eq!(
                    (to, amount),
                    (rejected.to, rejected.reimbursed_amount),
                    "Attempted to refund another account or amount than recorded for the failed SPL withdrawal request {burn_index}"
                );
            }
            SplRefundKind::UnusedRent => {
                let successful = self
                    .successful_spl_withdrawal_requests
                    .get(&burn_index)
                    .unwrap_or_else(|| {
                        panic!("Attempted to refund the rent of SPL withdrawal {burn_index} that has no transfer outcome")
                    });
                assert_eq!(
                    amount,
                    successful
                        .request
                        .terms
                        .rent_reserve
                        .saturating_sub(successful.rent_spent),
                    "Attempted to refund another amount than the unused rent of SPL withdrawal {burn_index}"
                );
            }
            SplRefundKind::FailedTransferRent => {
                let failed = self
                    .failed_spl_withdrawal_requests
                    .get(&burn_index)
                    .unwrap_or_else(|| {
                        panic!("Attempted to refund the rent of SPL withdrawal {burn_index} that has no transfer outcome")
                    });
                assert_eq!(
                    amount, failed.request.terms.rent_reserve,
                    "Attempted to refund another amount than the rent reserve of SPL withdrawal {burn_index}"
                );
            }
        }
        self.pending_spl_cksol_refunds.insert(
            *burn_index.get(),
            PendingSplCkSolRefund {
                kind,
                to,
                amount,
                created_at_time,
            },
        );
    }

    fn take_pending_spl_cksol_refund(
        &mut self,
        burn_index: LedgerBurnIndex,
        kind: SplRefundKind,
    ) -> PendingSplCkSolRefund {
        let pending = self
            .pending_spl_cksol_refunds
            .remove(burn_index.get())
            .unwrap_or_else(|| {
                panic!("Attempted to resolve a ckSOL refund of SPL withdrawal {burn_index} that is not pending")
            });
        assert_eq!(
            pending.kind, kind,
            "Attempted to resolve a ckSOL refund of SPL withdrawal {burn_index} with another kind"
        );
        pending
    }

    fn process_reimbursed_spl_cksol(
        &mut self,
        burn_index: LedgerBurnIndex,
        kind: SplRefundKind,
        mint_block_index: &LedgerMintIndex,
        reimbursed_at: u64,
    ) {
        let refund = self.take_pending_spl_cksol_refund(burn_index, kind);
        self.completed_spl_cksol_refunds.insert(
            *burn_index.get(),
            CompletedSplCkSolRefund {
                refund,
                mint_block_index: *mint_block_index,
                reimbursed_at,
            },
        );
    }

    fn process_quarantined_spl_cksol_refund(
        &mut self,
        burn_index: LedgerBurnIndex,
        kind: SplRefundKind,
    ) {
        let refund = self.take_pending_spl_cksol_refund(burn_index, kind);
        self.quarantined_spl_cksol_refunds
            .insert(*burn_index.get(), refund);
    }

    fn assert_valid_spl_withdrawal_terms(&self, request: &SplWithdrawalRequestTerms) {
        let token = self
            .supported_spl_tokens
            .get(&request.mint)
            .expect("Attempted to accept an SPL withdrawal for an unsupported mint");
        assert_eq!(
            token.ledger_id, request.ledger_id,
            "Attempted to accept an SPL withdrawal with a ledger different from the mint's registered ledger"
        );
        assert!(
            request.amount > request.token_fee,
            "Attempted to accept an SPL withdrawal that transfers no tokens"
        );
        assert!(
            request.fee > 0,
            "Attempted to accept an SPL withdrawal with a zero fee burn"
        );
        assert!(
            request.cksol_burn_amount().is_some(),
            "Attempted to accept an SPL withdrawal whose fee and rent reserve overflow"
        );
    }

    fn has_spl_withdrawal_outcome(&self, burn_index: &LedgerBurnIndex) -> bool {
        self.pending_spl_withdrawal_requests
            .contains_key(burn_index)
            || self
                .created_spl_withdrawal_requests
                .contains_key(burn_index)
            || self.sent_spl_withdrawal_requests.contains_key(burn_index)
            || self
                .successful_spl_withdrawal_requests
                .contains_key(burn_index)
            || self.failed_spl_withdrawal_requests.contains_key(burn_index)
            || self
                .rejected_spl_withdrawal_requests
                .contains_key(burn_index)
    }

    fn process_accepted_spl_withdrawal(&mut self, request: &SplWithdrawalRequest, created_at: u64) {
        let cksol_burn_block_index = request.cksol_burn_block_index;
        self.assert_valid_spl_withdrawal_terms(&request.terms);
        assert!(
            !self.has_spl_withdrawal_outcome(&cksol_burn_block_index),
            "Attempted to accept an already accepted SPL withdrawal request: {cksol_burn_block_index}"
        );
        let token_burn = (request.terms.ledger_id, request.ckspl_burn_block_index);
        assert!(
            !self.spl_withdrawal_token_burns.contains(&token_burn),
            "Attempted to reuse a token burn that already belongs to a withdrawal: {token_burn:?}"
        );
        self.spl_withdrawal_token_burns.insert(token_burn);
        self.pending_spl_withdrawal_requests.insert(
            cksol_burn_block_index,
            PendingSplWithdrawalRequest {
                request: request.clone(),
                created_at,
            },
        );
    }

    fn process_failed_spl_withdrawal(
        &mut self,
        burn_index: LedgerBurnIndex,
        to: Account,
        reimbursed_amount: Lamport,
    ) {
        assert!(
            !self.has_spl_withdrawal_outcome(&burn_index),
            "Attempted to record an already recorded SPL withdrawal request as failed: {burn_index}"
        );
        assert!(
            reimbursed_amount > 0,
            "Attempted to reimburse an SPL withdrawal with a zero amount"
        );
        self.rejected_spl_withdrawal_requests.insert(
            burn_index,
            RejectedSplWithdrawalRequest {
                to,
                reimbursed_amount,
            },
        );
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
        timestamp: u64,
    ) {
        assert!(
            !self.succeeded_transactions.contains(signature),
            "Attempted to submit already succeeded transaction {signature:?}"
        );
        assert!(
            !self.failed_transactions.contains_key(signature),
            "Attempted to submit already failed transaction {signature:?}"
        );
        let submitted_transaction = match purpose {
            TransactionPurpose::SweepSplDeposit {
                deposit_id,
                block_height,
            } => {
                self.sweep_spl_deposit(signature, *deposit_id, transaction, signers);
                MinterTransaction::SweepSplDeposit {
                    block_height: *block_height,
                }
            }
            TransactionPurpose::Withdrawal { burn_indices } => self.send_nonce_withdrawal(
                signature,
                transaction.clone(),
                signers,
                burn_indices,
                timestamp,
            ),
            TransactionPurpose::SplWithdrawal {
                cksol_burn_block_index,
            } => self.send_spl_nonce_withdrawal(
                signature,
                transaction.clone(),
                signers,
                *cksol_burn_block_index,
                timestamp,
            ),
            TransactionPurpose::SweepDeposit {
                deposit_ids,
                block_height,
            } => {
                let sweep_destination = minter_address(self.minter_public_key.as_ref().expect(
                    "BUG: a sweep was submitted before the minter public key was recorded",
                ));
                self.deposits
                    .sweep(deposit_ids, sweep_destination, transaction, signature);
                MinterTransaction::SweepDeposit {
                    block_height: *block_height,
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

    fn process_spl_transaction_created(
        &mut self,
        burn_index: LedgerBurnIndex,
        nonce_account: &Address,
        nonce_value: Hash,
    ) {
        self.nonce_pool.bind(nonce_account, nonce_value);
        let pending = self
            .pending_spl_withdrawal_requests
            .remove(&burn_index)
            .unwrap_or_else(|| {
                panic!("Attempted to create transaction for unknown SPL withdrawal request: {burn_index}")
            });
        let terms = &pending.request.terms;
        // The SOL for the transaction fee and the rent, and the tokens to transfer, stay
        // reserved until the outcome of the transaction is settled.
        self.balance = self
            .balance
            .checked_sub(spl_withdrawal_sol_cost(terms))
            .expect("BUG: insufficient minter balance for SPL withdrawal");
        let treasury = self.spl_treasury_balances.entry(terms.mint).or_default();
        *treasury = treasury
            .checked_sub(spl_withdrawal_token_cost(terms))
            .expect("BUG: insufficient treasury tokens for SPL withdrawal");
        assert_eq!(
            self.created_spl_withdrawal_requests
                .insert(burn_index, pending),
            None,
            "Attempted to create transaction for already created SPL withdrawal request: {burn_index}"
        );
        assert_eq!(
            self.created_spl_withdrawal_txs.insert(
                *nonce_account,
                CreatedSplWithdrawalTransaction {
                    nonce_value,
                    cksol_burn_block_index: burn_index,
                }
            ),
            None,
            "BUG: nonce account {nonce_account} already has a created SPL transaction"
        );
    }

    fn send_spl_nonce_withdrawal(
        &mut self,
        signature: &Signature,
        message: VersionedMessage,
        signers: &[Signer],
        burn_index: LedgerBurnIndex,
        submitted_at: u64,
    ) -> MinterTransaction {
        let nonce_account = message.advanced_nonce_account().unwrap_or_else(|| {
            panic!("BUG: SPL withdrawal transaction {signature} does not start with an AdvanceNonceAccount instruction")
        });
        let created = self
            .created_spl_withdrawal_txs
            .get(&nonce_account)
            .unwrap_or_else(|| {
                panic!(
                    "BUG: no SPL withdrawal transaction was created for nonce account {nonce_account}"
                )
            })
            .clone();
        assert_eq!(
            burn_index, created.cksol_burn_block_index,
            "BUG: SPL withdrawal transaction {signature} does not serve the request bound to nonce account {nonce_account}"
        );
        let bound = self
            .bound_spl_withdrawal_message(&nonce_account, &created)
            .unwrap_or_else(|e| {
                panic!("BUG: cannot rebuild the SPL withdrawal message bound to nonce account {nonce_account}: {e}")
            });
        assert_eq!(
            message,
            VersionedMessage::Legacy(bound),
            "BUG: SPL withdrawal transaction {signature} does not carry the message bound to nonce account {nonce_account}"
        );
        assert_eq!(
            signers,
            [Signer::Minter],
            "BUG: SPL withdrawal transaction {signature} must be signed by the minter only"
        );
        self.created_spl_withdrawal_txs.remove(&nonce_account);
        let pending = self
            .created_spl_withdrawal_requests
            .remove(&burn_index)
            .unwrap_or_else(|| {
                panic!("BUG: SPL withdrawal request {burn_index} of a created transaction is not in the created bucket")
            });
        assert_eq!(
            self.sent_spl_withdrawal_requests.insert(
                burn_index,
                SentSplWithdrawalRequest {
                    request: pending.request,
                    signature: *signature,
                    created_at: pending.created_at,
                },
            ),
            None,
            "Attempted to send transaction for already sent SPL withdrawal request: {burn_index}"
        );
        MinterTransaction::SplWithdrawal {
            message,
            nonce_account,
            nonce_value: created.nonce_value,
            submitted_at,
        }
    }

    /// Rebuilds the message of the SPL withdrawal bound to this nonce account from
    /// recorded state only, so a new signing attempt and replay produce the same message.
    pub fn bound_spl_withdrawal_message(
        &self,
        nonce_account: &Address,
        created: &CreatedSplWithdrawalTransaction,
    ) -> Result<Message, CreateTransferError> {
        let burn_index = created.cksol_burn_block_index;
        let request = &self
            .created_spl_withdrawal_requests
            .get(&burn_index)
            .unwrap_or_else(|| {
                panic!("BUG: SPL withdrawal request {burn_index} of a created transaction is not in the created bucket")
            })
            .request;
        let minter_address = minter_address(self.minter_public_key.as_ref().expect(
            "BUG: an SPL withdrawal message was rebuilt before the minter public key was recorded",
        ));
        self.spl_withdrawal_message(&minter_address, nonce_account, created.nonce_value, request)
    }

    /// The message that transfers the net amount of the request to its destination.
    pub fn spl_withdrawal_message(
        &self,
        minter_address: &Address,
        nonce_account: &Address,
        nonce_value: Hash,
        request: &SplWithdrawalRequest,
    ) -> Result<Message, CreateTransferError> {
        let terms = &request.terms;
        let token = self
            .supported_spl_tokens
            .get(&terms.mint)
            .expect("BUG: an accepted SPL withdrawal has a registered mint");
        build_spl_withdrawal_message(
            minter_address,
            nonce_account,
            nonce_value,
            &SplWithdrawalTransfer {
                token,
                destination: &terms.address,
                amount: spl_withdrawal_token_cost(terms),
            },
        )
    }

    fn process_transaction_created(
        &mut self,
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
            total = total
                .checked_add(pending.request.amount_to_transfer)
                .expect("BUG: total amount of a withdrawal transaction overflows");
            assert_eq!(
                self.created_withdrawal_requests
                    .insert(*burn_index, pending),
                None,
                "Attempted to create transaction for already created withdrawal request: {burn_index:?}"
            );
        }
        self.balance = self
            .balance
            .checked_sub(total + BATCH_WITHDRAWAL_TX_FEE)
            .expect("BUG: insufficient minter balance for withdrawal");
        assert_eq!(
            self.created_withdrawal_txs.insert(
                *nonce_account,
                CreatedWithdrawalTransaction {
                    nonce_value,
                    burn_indices: burn_indices.to_vec(),
                }
            ),
            None,
            "BUG: nonce account {nonce_account} already has a created transaction"
        );
    }

    fn send_nonce_withdrawal(
        &mut self,
        signature: &Signature,
        message: VersionedMessage,
        signers: &[Signer],
        burn_indices: &[LedgerBurnIndex],
        submitted_at: u64,
    ) -> MinterTransaction {
        let nonce_account = message.advanced_nonce_account().unwrap_or_else(|| {
            panic!("BUG: withdrawal transaction {signature} does not start with an AdvanceNonceAccount instruction")
        });
        let created = self
            .created_withdrawal_txs
            .remove(&nonce_account)
            .unwrap_or_else(|| {
                panic!(
                    "BUG: no withdrawal transaction was created for nonce account {nonce_account}"
                )
            });
        assert_eq!(
            burn_indices, created.burn_indices,
            "BUG: withdrawal transaction {signature} does not serve the withdrawal requests bound to nonce account {nonce_account}"
        );
        assert_eq!(
            message,
            VersionedMessage::Legacy(self.bound_withdrawal_message(&nonce_account, &created)),
            "BUG: withdrawal transaction {signature} does not carry the message bound to nonce account {nonce_account}"
        );
        assert_eq!(
            signers,
            [Signer::Minter],
            "BUG: withdrawal transaction {signature} must be signed by the minter only"
        );
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
        MinterTransaction::Withdrawal {
            message,
            nonce_account,
            nonce_value: created.nonce_value,
            submitted_at,
        }
    }

    fn bound_withdrawal_message(
        &self,
        nonce_account: &Address,
        created: &CreatedWithdrawalTransaction,
    ) -> Message {
        let minter_address =
            minter_address(self.minter_public_key.as_ref().expect(
                "BUG: a withdrawal was submitted before the minter public key was recorded",
            ));
        let transfers: Vec<(Address, Lamport)> = created
            .burn_indices
            .iter()
            .map(|burn_index| {
                let request = &self
                    .created_withdrawal_requests
                    .get(burn_index)
                    .unwrap_or_else(|| {
                        panic!(
                            "BUG: withdrawal request {burn_index:?} of a created transaction is not in the created bucket"
                        )
                    })
                    .request;
                (
                    Address::from(request.solana_address),
                    request.amount_to_transfer,
                )
            })
            .collect();
        build_batch_withdrawal_message(
            &minter_address,
            nonce_account,
            created.nonce_value,
            &transfers,
        )
        .unwrap_or_else(|e| {
            panic!("BUG: cannot rebuild the withdrawal message bound to nonce account {nonce_account}: {e}")
        })
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
            MinterTransaction::SweepSplDeposit { .. } => {
                self.spl_deposits.finalize_swept(signature)
            }
            MinterTransaction::Withdrawal { nonce_account, .. } => {
                self.nonce_pool.free(&nonce_account)
            }
            MinterTransaction::SplWithdrawal { .. } => {
                panic!("BUG: SPL withdrawal transaction {signature} is settled by its own events")
            }
        }
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
            // The cost of a failed SPL sweep is unknown here. `FailedSplSweep` settles it.
            MinterTransaction::SweepSplDeposit { .. } => self.spl_deposits.fail_swept(signature),
            MinterTransaction::Withdrawal { nonce_account, .. } => {
                self.nonce_pool.free(nonce_account)
            }
            MinterTransaction::SplWithdrawal { .. } => {
                panic!("BUG: SPL withdrawal transaction {signature} is settled by its own events")
            }
        }
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
    AutomaticDepositFeeTooHigh {
        automatic_deposit_fee: Lamport,
        max_automatic_deposit_fee: Lamport,
    },
    InvalidNonceAccount(String),
    DuplicateNonceAccount(Address),
    NonceAccountIsWithdrawalDestination(Address),
    InvalidSplToken(String),
    InvalidOperatingRefillConfig,
    SplWithdrawalFeeTooLow {
        spl_withdrawal_fee: Lamport,
        transaction_fee: Lamport,
    },
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
            automatic_deposit_fee,
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
            automatic_deposit_fee,
            collected_automatic_deposit_fees: 0,
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
            succeeded_transactions: BTreeSet::new(),
            failed_transactions: InsertionOrderedMap::new(),
            nonce_pool,
            active_tasks: BTreeSet::new(),
            balance: 0,
            supported_spl_tokens: BTreeMap::new(),
            operating_balance: operating_balance::OperatingBalance::default(),
            spl_deposits: SplDeposits::default(),
            spl_deposit_consolidation_fee: crate::constants::DEFAULT_SPL_DEPOSIT_CONSOLIDATION_FEE,
            spl_withdrawal_fee: crate::constants::DEFAULT_SPL_WITHDRAWAL_FEE,
            spl_treasury_balances: BTreeMap::new(),
            pending_spl_withdrawal_requests: BTreeMap::new(),
            created_spl_withdrawal_requests: BTreeMap::new(),
            created_spl_withdrawal_txs: BTreeMap::new(),
            sent_spl_withdrawal_requests: BTreeMap::new(),
            successful_spl_withdrawal_requests: BTreeMap::new(),
            failed_spl_withdrawal_requests: BTreeMap::new(),
            pending_spl_withdrawal_reimbursements: BTreeMap::new(),
            completed_spl_withdrawal_reimbursements: BTreeMap::new(),
            quarantined_spl_withdrawal_reimbursements: BTreeMap::new(),
            collected_spl_withdrawal_fees: BTreeMap::new(),
            spl_withdrawal_token_burns: BTreeSet::new(),
            rejected_spl_withdrawal_requests: BTreeMap::new(),
            pending_spl_cksol_refunds: BTreeMap::new(),
            completed_spl_cksol_refunds: BTreeMap::new(),
            quarantined_spl_cksol_refunds: BTreeMap::new(),
            pending_deposit_spl_request_guards: BTreeSet::new(),
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

/// An accepted SPL withdrawal with both burns recorded, waiting for processing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingSplWithdrawalRequest {
    pub request: SplWithdrawalRequest,
    pub created_at: u64,
}

/// The binding of a durable nonce account and its nonce value to one SPL withdrawal
/// request, as [`CreatedWithdrawalTransaction`] is for SOL withdrawals.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CreatedSplWithdrawalTransaction {
    pub nonce_value: Hash,
    pub cksol_burn_block_index: LedgerBurnIndex,
}

/// An SPL withdrawal request that has been submitted in a Solana transaction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SentSplWithdrawalRequest {
    pub request: SplWithdrawalRequest,
    pub signature: Signature,
    pub created_at: u64,
}

/// An SPL withdrawal whose transfer succeeded, with its validated cost.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SuccessfulSplWithdrawalRequest {
    pub request: SplWithdrawalRequest,
    pub signature: Signature,
    pub created_at: u64,
    /// The lamports that the main account spent: the transaction fee and the rent.
    pub lamports_spent: Lamport,
    /// The part of `lamports_spent` that created the destination token account.
    pub rent_spent: Lamport,
}

/// A failed SPL withdrawal whose burned tokens wait for their remint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingSplWithdrawalReimbursement {
    pub request: SplWithdrawalRequest,
    pub created_at: u64,
}

/// A failed SPL withdrawal whose burned tokens were reminted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompletedSplWithdrawalReimbursement {
    pub request: SplWithdrawalRequest,
    pub mint_block_index: LedgerMintIndex,
    pub created_at: u64,
    pub reimbursed_at: u64,
}

/// The SOL that the transaction of an SPL withdrawal can spend: its fee and the rent of
/// the destination token account.
fn spl_withdrawal_sol_cost(terms: &SplWithdrawalRequestTerms) -> Lamport {
    BATCH_WITHDRAWAL_TX_FEE
        .checked_add(terms.rent_reserve)
        .expect("BUG: SPL withdrawal SOL cost overflows")
}

/// The tokens that the transaction of an SPL withdrawal takes from the treasury.
fn spl_withdrawal_token_cost(terms: &SplWithdrawalRequestTerms) -> u64 {
    terms
        .amount_to_transfer()
        .expect("BUG: an accepted SPL withdrawal transfers a positive amount")
}

/// An SPL withdrawal request whose token burn failed, with the refund of its fee burn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RejectedSplWithdrawalRequest {
    pub to: Account,
    pub reimbursed_amount: Lamport,
}

/// A ckSOL refund of an SPL withdrawal that waits for its mint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingSplCkSolRefund {
    pub kind: SplRefundKind,
    pub to: Account,
    pub amount: Lamport,
    pub created_at_time: u64,
}

/// A ckSOL refund of an SPL withdrawal that was minted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompletedSplCkSolRefund {
    pub refund: PendingSplCkSolRefund,
    pub mint_block_index: LedgerMintIndex,
    pub reimbursed_at: u64,
}

/// A withdrawal transaction to create. SOL requests are batched; an SPL request has a
/// transaction of its own.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WithdrawalPlan {
    Sol(Vec<WithdrawalRequest>),
    Spl(SplWithdrawalRequest),
}

/// Groups pending withdrawal requests, oldest ckSOL burn index first, into transactions
/// that the minter can pay for: SOL requests in batches with one transaction fee per
/// batch, and each SPL request alone with its fee, its rent and its treasury tokens.
/// The rent exemption threshold is held back, so a withdrawal transaction can never
/// leave the main address below it.
///
/// SOL requests are never reordered or skipped: the first one that the remaining
/// balance cannot cover stops the SOL requests. An SPL request that the funds cannot
/// cover is skipped, so it blocks neither later SPL requests nor SOL requests.
pub struct WithdrawalPlans<'a> {
    pending_requests: Peekable<btree_map::Values<'a, LedgerBurnIndex, PendingWithdrawalRequest>>,
    pending_spl_requests:
        Peekable<btree_map::Values<'a, LedgerBurnIndex, PendingSplWithdrawalRequest>>,
    available_balance: Lamport,
    available_tokens: BTreeMap<Address, u64>,
    sol_blocked: bool,
}

impl Iterator for WithdrawalPlans<'_> {
    type Item = WithdrawalPlan;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let next_sol = self.next_burn_index_of_sol();
            let next_spl = self.next_burn_index_of_spl();
            let plan = match (next_sol, next_spl) {
                (None, None) => return None,
                (Some(sol), Some(spl)) if spl < sol => self.next_spl_request(),
                (Some(_), _) => self.next_sol_batch(next_spl),
                (None, Some(_)) => self.next_spl_request(),
            };
            if plan.is_some() {
                return plan;
            }
        }
    }
}

impl WithdrawalPlans<'_> {
    fn next_burn_index_of_sol(&mut self) -> Option<LedgerBurnIndex> {
        if self.sol_blocked {
            return None;
        }
        self.pending_requests
            .peek()
            .map(|pending| pending.request.burn_block_index)
    }

    fn next_burn_index_of_spl(&mut self) -> Option<LedgerBurnIndex> {
        self.pending_spl_requests
            .peek()
            .map(|pending| pending.request.cksol_burn_block_index)
    }

    /// Batches SOL requests up to the next SPL request, so a batch never serves a
    /// request newer than an SPL request still waiting.
    fn next_sol_batch(&mut self, next_spl: Option<LedgerBurnIndex>) -> Option<WithdrawalPlan> {
        let mut batch = Vec::new();
        while batch.len() < MAX_WITHDRAWALS_PER_NONCE_TX {
            let reserved_fee = if batch.is_empty() {
                BATCH_WITHDRAWAL_TX_FEE
            } else {
                0
            };
            let Some(pending) = self.pending_requests.peek() else {
                break;
            };
            if next_spl.is_some_and(|spl| spl < pending.request.burn_block_index) {
                break;
            }
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
        if batch.is_empty() {
            self.sol_blocked = true;
            None
        } else {
            Some(WithdrawalPlan::Sol(batch))
        }
    }

    /// Takes the next SPL request and reserves its funds, or skips it if they do not
    /// cover it.
    fn next_spl_request(&mut self) -> Option<WithdrawalPlan> {
        let request = &self.pending_spl_requests.next()?.request;
        let terms = &request.terms;
        let remaining_balance = self
            .available_balance
            .checked_sub(spl_withdrawal_sol_cost(terms))?;
        let tokens = self.available_tokens.get(&terms.mint).copied().unwrap_or(0);
        let remaining_tokens = tokens.checked_sub(spl_withdrawal_token_cost(terms))?;
        self.available_balance = remaining_balance;
        self.available_tokens.insert(terms.mint, remaining_tokens);
        Some(WithdrawalPlan::Spl(request.clone()))
    }
}

/// The binding of a durable nonce account and its nonce value to the
/// withdrawal requests of a transaction whose threshold signature was not yet
/// recorded. The binding determines the transaction message, so that a signing
/// failure leads to re-signing the identical message instead of building a new
/// one for the same nonce value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CreatedWithdrawalTransaction {
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
    WithdrawalProcessing,
    ScanWatchedAddresses,
    OperatingRefill,
    SweepSplDeposits,
    MintSpl,
    RefundSplCkSol,
    ReimburseSplWithdrawals,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MinterTransaction {
    SweepDeposit {
        /// The block height of the block whose blockhash the transaction uses.
        block_height: BlockHeight,
    },
    SweepSplDeposit {
        /// The block height of the block whose blockhash the transaction uses.
        block_height: BlockHeight,
    },
    /// A durable-nonce withdrawal transaction, which never expires: it stays
    /// in flight until it is finalized.
    Withdrawal {
        message: VersionedMessage,
        /// The durable nonce account whose nonce value the transaction uses.
        nonce_account: Address,
        /// The durable nonce value the transaction uses instead of a recent blockhash.
        nonce_value: Hash,
        /// The time, in nanoseconds since the epoch, at which the transaction was recorded as submitted.
        submitted_at: u64,
    },
    /// A durable-nonce transaction of one SPL withdrawal. Like [`Self::Withdrawal`], it
    /// never expires. Its outcome is settled by its own events.
    SplWithdrawal {
        message: VersionedMessage,
        /// The durable nonce account whose nonce value the transaction uses.
        nonce_account: Address,
        /// The durable nonce value the transaction uses instead of a recent blockhash.
        nonce_value: Hash,
        /// The time, in nanoseconds since the epoch, at which the transaction was recorded as submitted.
        submitted_at: u64,
    },
}

/// The part of the reservation of a sweep that its actual cost leaves unspent.
fn unspent_spl_sweep_cost(sweep: &SubmittedSplSweep, lamports_spent: Lamport) -> Lamport {
    sweep
        .reserved_lamports
        .checked_sub(lamports_spent)
        .expect("Attempted to settle an SPL sweep above its reserved cost")
}
