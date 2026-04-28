use super::{signer::MockSchnorrSigner, stubs::Stubs};
use crate::{runtime::CanisterRuntime, signer::SchnorrSigner};
use candid::{CandidType, Nat, Principal};
use ic_canister_runtime::{IcError, Runtime, StubRuntime};
use ic_cdk_management_canister::{SchnorrPublicKeyArgs, SchnorrPublicKeyResult, SignCallError};
use icrc_ledger_types::icrc1::transfer::{BlockIndex, TransferError};
use icrc_ledger_types::icrc2::transfer_from::TransferFromError;
use sol_rpc_types::{
    ConfirmedBlock, ConfirmedTransactionStatusWithSignature,
    EncodedConfirmedTransactionWithStatusMeta, MultiRpcResult, RpcError,
    Signature as SolRpcSignature, Slot, TransactionStatus,
};
use std::{
    future::Future,
    sync::{Arc, Mutex},
    time::Duration,
};

pub const TEST_CANISTER_ID: Principal = Principal::from_slice(&[0xCA; 10]);

#[derive(Clone, Default)]
pub struct TestCanisterRuntime {
    inter_canister_call_runtime: StubRuntime,
    signer: MockSchnorrSigner,
    times: Stubs<u64>,
    instruction_counts: Stubs<u64>,
    msg_cycles_accept: Stubs<u128>,
    msg_cycles_available: Stubs<u128>,
    msg_cycles_refunded: Stubs<u128>,
    set_timer_call_count: Arc<Mutex<usize>>,
    schnorr_public_key_results: Stubs<SchnorrPublicKeyResult>,
}

impl TestCanisterRuntime {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_stub_response<Out: CandidType>(mut self, response: Out) -> Self {
        self.inter_canister_call_runtime =
            self.inter_canister_call_runtime.add_stub_response(response);
        self
    }

    pub fn add_stub_error(mut self, error: IcError) -> Self {
        self.inter_canister_call_runtime = self.inter_canister_call_runtime.add_stub_error(error);
        self
    }

    pub fn with_time(mut self, timestamp: u64) -> Self {
        self.times = self.times.add(timestamp);
        self
    }

    pub fn add_times<I>(mut self, times: I) -> Self
    where
        I: IntoIterator<Item = u64>,
    {
        for time in times {
            self.times = self.times.add(time);
        }
        self
    }

    pub fn with_increasing_time(mut self) -> Self {
        self.times = (0..).into();
        self
    }

    pub fn add_msg_cycles_accept(mut self, value: u128) -> Self {
        self.msg_cycles_accept = self.msg_cycles_accept.add(value);
        self
    }

    pub fn add_msg_cycles_available(mut self, value: u128) -> Self {
        self.msg_cycles_available = self.msg_cycles_available.add(value);
        self
    }

    pub fn add_msg_cycles_refunded(mut self, value: u128) -> Self {
        self.msg_cycles_refunded = self.msg_cycles_refunded.add(value);
        self
    }

    pub fn add_signature(mut self, signature: [u8; 64]) -> Self {
        self.signer = self.signer.add_signature(signature);
        self
    }

    pub fn add_schnorr_signing_error(mut self, error: SignCallError) -> Self {
        self.signer = self.signer.add_response(Err(error));
        self
    }

    pub fn with_schnorr_public_key(mut self, result: SchnorrPublicKeyResult) -> Self {
        self.schnorr_public_key_results = self.schnorr_public_key_results.add(result);
        self
    }

    // ── getTransaction ────────────────────────────────────────────────────────

    /// Stubs the next `getTransaction` JSON-RPC call to return the given transaction.
    pub fn add_get_transaction_response(
        self,
        tx: impl TryInto<EncodedConfirmedTransactionWithStatusMeta>,
    ) -> Self {
        self.add_stub_response(MultiRpcResult::<
            Option<EncodedConfirmedTransactionWithStatusMeta>,
        >::Consistent(Ok(Some(
            tx.try_into().ok().expect("failed to convert transaction"),
        ))))
    }

    /// Stubs the next `getTransaction` JSON-RPC call to return `None` (transaction not found).
    pub fn add_get_transaction_not_found(self) -> Self {
        self.add_stub_response(MultiRpcResult::<
            Option<EncodedConfirmedTransactionWithStatusMeta>,
        >::Consistent(Ok(None)))
    }

    /// Stubs `n` consecutive `getTransaction` calls to return `None`.
    pub fn add_n_get_transaction_not_found(self, n: usize) -> Self {
        (0..n).fold(self, |rt, _| rt.add_get_transaction_not_found())
    }

    // ── getSignaturesForAddress ───────────────────────────────────────────────

    /// Stubs the next `getSignaturesForAddress` JSON-RPC call to return the given signatures.
    pub fn add_get_signatures_for_address_response(
        self,
        sigs: Vec<ConfirmedTransactionStatusWithSignature>,
    ) -> Self {
        self.add_stub_response(
            MultiRpcResult::<Vec<ConfirmedTransactionStatusWithSignature>>::Consistent(Ok(sigs)),
        )
    }

    /// Stubs the next `getSignaturesForAddress` JSON-RPC call to return an error.
    pub fn add_get_signatures_for_address_error(self, err: RpcError) -> Self {
        self.add_stub_response(
            MultiRpcResult::<Vec<ConfirmedTransactionStatusWithSignature>>::Consistent(Err(err)),
        )
    }

    // ── getSlot ───────────────────────────────────────────────────────────────

    /// Stubs the next `getSlot` JSON-RPC call to return the given slot.
    pub fn add_get_slot_response(self, slot: Slot) -> Self {
        self.add_stub_response(MultiRpcResult::<Slot>::Consistent(Ok(slot)))
    }

    /// Stubs the next `getSlot` JSON-RPC call to return an error.
    pub fn add_get_slot_error(self, err: RpcError) -> Self {
        self.add_stub_response(MultiRpcResult::<Slot>::Consistent(Err(err)))
    }

    /// Stubs `n` consecutive `getSlot` JSON-RPC calls to return the given error.
    pub fn add_n_get_slot_error(self, err: RpcError, n: usize) -> Self {
        (0..n).fold(self, |rt, _| rt.add_get_slot_error(err.clone()))
    }

    // ── getBlock ──────────────────────────────────────────────────────────────

    /// Stubs the next `getBlock` JSON-RPC call to return the given block.
    pub fn add_get_block_response(self, block: ConfirmedBlock) -> Self {
        self.add_stub_response(MultiRpcResult::<ConfirmedBlock>::Consistent(Ok(block)))
    }

    // ── sendTransaction ───────────────────────────────────────────────────────

    /// Stubs the next `sendTransaction` JSON-RPC call to return the given signature.
    pub fn add_send_transaction_response(self, sig: impl Into<SolRpcSignature>) -> Self {
        self.add_stub_response(MultiRpcResult::<SolRpcSignature>::Consistent(
            Ok(sig.into()),
        ))
    }

    // ── getSignatureStatuses ──────────────────────────────────────────────────

    /// Stubs the next `getSignatureStatuses` JSON-RPC call to return the given statuses.
    pub fn add_get_signature_statuses_response(
        self,
        statuses: Vec<Option<TransactionStatus>>,
    ) -> Self {
        self.add_stub_response(
            MultiRpcResult::<Vec<Option<TransactionStatus>>>::Consistent(Ok(statuses)),
        )
    }

    /// Stubs the next `getSignatureStatuses` JSON-RPC call to return an error.
    pub fn add_get_signature_statuses_error(self, err: RpcError) -> Self {
        self.add_stub_response(
            MultiRpcResult::<Vec<Option<TransactionStatus>>>::Consistent(Err(err)),
        )
    }

    // ── Ledger ────────────────────────────────────────────────────────────────

    /// Stubs the next `icrc1_transfer` ledger call (used to mint ckSOL).
    pub fn add_icrc1_transfer_response(self, result: Result<BlockIndex, TransferError>) -> Self {
        self.add_stub_response(result)
    }

    /// Stubs the next `icrc2_transfer_from` ledger call (used to burn ckSOL when withdrawing).
    pub fn add_icrc2_transfer_from_response(self, result: Result<Nat, TransferFromError>) -> Self {
        self.add_stub_response(result)
    }

    #[cfg(any(test, not(feature = "canbench-rs")))]
    pub(crate) fn set_timer_call_count(&self) -> usize {
        *self.set_timer_call_count.lock().unwrap()
    }
}

impl CanisterRuntime for TestCanisterRuntime {
    fn inter_canister_call_runtime(&self) -> impl Runtime {
        // This clone returns a new reference to the same stubs
        self.inter_canister_call_runtime.clone()
    }

    fn signer(&self) -> impl SchnorrSigner {
        self.signer.clone()
    }

    fn canister_self(&self) -> Principal {
        TEST_CANISTER_ID
    }

    fn time(&self) -> u64 {
        self.times.next()
    }

    fn instruction_counter(&self) -> u64 {
        self.instruction_counts.next()
    }

    fn msg_cycles_accept(&self, amount: u128) -> u128 {
        assert_eq!(self.msg_cycles_accept.next(), amount);
        amount
    }

    fn msg_cycles_available(&self) -> u128 {
        self.msg_cycles_available.next()
    }

    fn msg_cycles_refunded(&self) -> u128 {
        self.msg_cycles_refunded.next()
    }

    fn set_timer<F, Fut>(&self, _delay: Duration, _f: F) -> ic_cdk_timers::TimerId
    where
        Self: Sized,
        F: FnOnce(Self) -> Fut + 'static,
        Fut: Future<Output = ()> + 'static,
    {
        *self.set_timer_call_count.lock().unwrap() += 1;
        Default::default()
    }

    async fn schnorr_public_key(&self, _args: SchnorrPublicKeyArgs) -> SchnorrPublicKeyResult {
        self.schnorr_public_key_results.next()
    }
}
