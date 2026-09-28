use super::{
    confirmed_block,
    signer::{MockSchnorrSigner, SignerExpectation, sign_for},
    stubs::Stubs,
};
use crate::{
    constants::GET_RECENT_BLOCK_MAX_TRIES, runtime::CanisterRuntime, signer::SchnorrSigner,
};
use async_trait::async_trait;
use candid::{
    CandidType, Principal,
    utils::{ArgumentEncoder, decode_args, encode_args},
};
use ic_canister_runtime::{IcError, Runtime, StubRuntime};
use ic_cdk::call::{CallPerformFailed, Error as CallError};
use ic_cdk_management_canister::{SchnorrPublicKeyArgs, SchnorrPublicKeyResult};
use icrc_ledger_types::icrc1::account::Account;
use serde::de::DeserializeOwned;
use sol_rpc_types::{MultiRpcResult, RpcResult, Signature, Slot};
use std::{
    future::Future,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::task::yield_now;

pub const TEST_CANISTER_ID: Principal = Principal::from_slice(&[0xCA; 10]);

#[derive(Clone, Default)]
pub struct TestCanisterRuntime {
    inter_canister_call_runtime: RecordingStubRuntime,
    signer: MockSchnorrSigner,
    times: Stubs<u64>,
    expects_charges: bool,
    msg_cycles_accepted: Arc<Mutex<Vec<u128>>>,
    msg_cycles_available: Stubs<u128>,
    msg_cycles_refunded: Stubs<u128>,
    set_timer_call_count: Arc<Mutex<usize>>,
    schnorr_public_key_results: Stubs<Result<SchnorrPublicKeyResult, CallError>>,
    schnorr_public_key_call_count: Arc<Mutex<usize>>,
}

impl TestCanisterRuntime {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers the mocks for one transaction paid for by `fee_payer` and submitted under
    /// `transaction_signature`. See [`TransactionBuilder`].
    pub fn transaction_builder(
        self,
        fee_payer: Account,
        transaction_signature: solana_signature::Signature,
    ) -> TransactionBuilder {
        TransactionBuilder::new(self, fee_payer, transaction_signature)
    }

    pub fn add_stub_response<Out: CandidType>(mut self, response: Out) -> Self {
        self.inter_canister_call_runtime.stub = self
            .inter_canister_call_runtime
            .stub
            .add_stub_response(response);
        self
    }

    pub fn add_stub_error(mut self, error: IcError) -> Self {
        self.inter_canister_call_runtime.stub =
            self.inter_canister_call_runtime.stub.add_stub_error(error);
        self
    }

    /// The inter-canister update calls made through this runtime, in call order.
    pub fn sent_update_calls(&self) -> Vec<SentUpdateCall> {
        self.inter_canister_call_runtime
            .sent_update_calls
            .lock()
            .unwrap()
            .clone()
    }

    pub fn add_recent_block(mut self, result: RpcResult<Slot>) -> Self {
        match result {
            Ok(slot) => self
                .add_stub_response(MultiRpcResult::Consistent(Ok(slot)))
                .add_stub_response(MultiRpcResult::Consistent(Ok(confirmed_block()))),
            Err(error) => {
                for _ in 0..GET_RECENT_BLOCK_MAX_TRIES.get() {
                    self = self
                        .add_stub_response(MultiRpcResult::<Slot>::Consistent(Err(error.clone())));
                }
                self
            }
        }
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

    pub fn expecting_charges(mut self) -> Self {
        self.expects_charges = true;
        self
    }

    pub fn msg_cycles_accepted(&self) -> Vec<u128> {
        self.msg_cycles_accepted.lock().unwrap().clone()
    }

    pub fn add_msg_cycles_available(mut self, value: u128) -> Self {
        self.msg_cycles_available = self.msg_cycles_available.add(value);
        self
    }

    pub fn add_msg_cycles_refunded(mut self, value: u128) -> Self {
        self.msg_cycles_refunded = self.msg_cycles_refunded.add(value);
        self
    }

    pub fn add_signer(mut self, expectation: SignerExpectation) -> Self {
        self.signer = self.signer.add_signer(expectation);
        self
    }

    pub fn with_schnorr_public_key(mut self, result: SchnorrPublicKeyResult) -> Self {
        self.schnorr_public_key_results = self.schnorr_public_key_results.add(Ok(result));
        self
    }

    pub fn with_schnorr_public_key_call_failure(mut self) -> Self {
        self.schnorr_public_key_results = self
            .schnorr_public_key_results
            .add(Err(CallPerformFailed.into()));
        self
    }

    pub(crate) fn set_timer_call_count(&self) -> usize {
        *self.set_timer_call_count.lock().unwrap()
    }

    pub(crate) fn schnorr_public_key_call_count(&self) -> usize {
        *self.schnorr_public_key_call_count.lock().unwrap()
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
        unimplemented!("TestCanisterRuntime does not model the instruction counter")
    }

    fn msg_cycles_accept(&self, amount: u128) -> u128 {
        assert!(
            self.expects_charges,
            "a test that does not expect the caller to be charged must not accept cycles, \
             but {amount} cycles were accepted: call \
             TestCanisterRuntime::expecting_charges() to opt in to being charged"
        );
        self.msg_cycles_accepted.lock().unwrap().push(amount);
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

    async fn schnorr_public_key(
        &self,
        _args: SchnorrPublicKeyArgs,
    ) -> Result<SchnorrPublicKeyResult, CallError> {
        *self.schnorr_public_key_call_count.lock().unwrap() += 1;
        suspend_like_an_inter_canister_call().await;
        self.schnorr_public_key_results.next()
    }
}

/// Wraps [`StubRuntime`] to record the method and Candid-encoded arguments of every
/// update call, so that a test can assert exactly what was sent.
#[derive(Clone, Default)]
struct RecordingStubRuntime {
    stub: StubRuntime,
    sent_update_calls: Arc<Mutex<Vec<SentUpdateCall>>>,
}

/// An inter-canister update call recorded by [`TestCanisterRuntime`].
#[derive(Clone, Debug, PartialEq)]
pub struct SentUpdateCall {
    pub method: String,
    args: Vec<u8>,
}

impl SentUpdateCall {
    /// Decodes the single Candid argument of the recorded call.
    pub fn single_arg<Arg: CandidType + DeserializeOwned>(&self) -> Arg {
        let (arg,) = decode_args(&self.args).expect("Failed to decode the call argument");
        arg
    }
}

#[async_trait]
impl Runtime for RecordingStubRuntime {
    async fn update_call<In, Out>(
        &self,
        id: Principal,
        method: &str,
        args: In,
        cycles: u128,
    ) -> Result<Out, IcError>
    where
        In: ArgumentEncoder + Send,
        Out: CandidType + DeserializeOwned,
    {
        self.sent_update_calls.lock().unwrap().push(SentUpdateCall {
            method: method.to_string(),
            args: encode_args(args).expect("Failed to encode the call arguments"),
        });
        self.stub.update_call(id, method, (), cycles).await
    }

    async fn query_call<In, Out>(
        &self,
        id: Principal,
        method: &str,
        args: In,
    ) -> Result<Out, IcError>
    where
        In: ArgumentEncoder + Send,
        Out: CandidType + DeserializeOwned,
    {
        self.stub.query_call(id, method, args).await
    }
}

/// Suspends the caller once, so that concurrent callers all reach the call before any of
/// them sees its response, as they do on the IC.
async fn suspend_like_an_inter_canister_call() {
    yield_now().await;
}

/// Expects the fee payer to sign with the transaction signature, answers the
/// `sendTransaction` call with it, and expects every account added with
/// [`Self::add_signers`] to sign with its own derived signature, which the test reads back
/// with `account_signature`.
///
/// Chain a further [`TestCanisterRuntime::transaction_builder`] onto [`Self::build`] for
/// each additional transaction a test expects.
pub struct TransactionBuilder(TestCanisterRuntime);

impl TransactionBuilder {
    fn new(
        runtime: TestCanisterRuntime,
        fee_payer: Account,
        transaction_signature: solana_signature::Signature,
    ) -> Self {
        Self(
            runtime
                .add_signer(sign_for(&fee_payer).expect([Ok(transaction_signature)]))
                .add_stub_response(MultiRpcResult::<Signature>::Consistent(Ok(
                    transaction_signature.into(),
                ))),
        )
    }

    pub fn add_signers(mut self, accounts: impl IntoIterator<Item = Account>) -> Self {
        for account in accounts {
            self.0 = self.0.add_signer(sign_for(&account));
        }
        self
    }

    pub fn build(self) -> TestCanisterRuntime {
        self.0
    }
}
