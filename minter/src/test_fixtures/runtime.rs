use crate::{address::DerivationPath, runtime::CanisterRuntime, sol_transfer::SchnorrSigner};
use candid::{CandidType, Principal};
use ic_canister_runtime::{IcError, Runtime, StubRuntime};
use ic_cdk::management_canister::SignCallError;
use std::{
    collections::VecDeque,
    iter,
    sync::{Arc, Mutex},
    time::Duration,
};

#[derive(Clone, Default)]
pub struct TestCanisterRuntime {
    inter_canister_call_runtime: StubRuntime,
    times: Stubs<u64>,
    instruction_counts: Stubs<u64>,
    msg_cycles_accept: Stubs<u128>,
    msg_cycles_available: Stubs<u128>,
    msg_cycles_refunded: Stubs<u128>,
    canister_self: Option<Principal>,
    schnorr_signer: MockSchnorrSigner,
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

    pub fn with_canister_self(mut self, canister_self: Principal) -> Self {
        self.canister_self = Some(canister_self);
        self
    }

    pub fn add_schnorr_signature(mut self, signature: [u8; 64]) -> Self {
        self.schnorr_signer.responses.add(Ok(signature.to_vec()));
        self
    }
}

impl CanisterRuntime for TestCanisterRuntime {
    fn inter_canister_call_runtime(&self) -> impl Runtime {
        // This clone returns a new reference to the same stubs
        self.inter_canister_call_runtime.clone()
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

    fn set_timer(
        &self,
        _delay: Duration,
        _future: impl Future<Output = ()> + 'static,
    ) -> ic_cdk_timers::TimerId {
        Default::default()
    }

    fn canister_self(&self) -> Principal {
        self.canister_self
            .expect("TestCanisterRuntime was not initialized with canister_self")
    }

    fn schnorr_signer(&self) -> impl SchnorrSigner {
        self.schnorr_signer.clone()
    }
}

#[derive(Clone, Default)]
pub struct MockSchnorrSigner {
    responses: SharedVecDeque<Result<Vec<u8>, SignCallError>>,
}

impl MockSchnorrSigner {
    pub fn with_signatures(signatures: Vec<[u8; 64]>) -> Self {
        Self {
            responses: SharedVecDeque::from_iter(
                signatures.into_iter().map(|sig| Ok(sig.to_vec())),
            ),
        }
    }

    pub fn with_responses(responses: Vec<Result<Vec<u8>, SignCallError>>) -> Self {
        Self {
            responses: SharedVecDeque::from_iter(responses),
        }
    }
}

impl SchnorrSigner for MockSchnorrSigner {
    async fn sign(
        &self,
        _message: Vec<u8>,
        _derivation_path: DerivationPath,
    ) -> Result<Vec<u8>, SignCallError> {
        self.responses
            .pop_front()
            .expect("MockSchnorrSigner: no more stub responses")
    }
}

#[derive(Clone)]
struct SharedVecDeque<T>(Arc<Mutex<VecDeque<T>>>);

impl<T> Default for SharedVecDeque<T> {
    fn default() -> Self {
        Self(Arc::new(Mutex::new(VecDeque::new())))
    }
}

impl<T> SharedVecDeque<T> {
    fn from_iter(iter: impl IntoIterator<Item = T>) -> Self {
        Self(Arc::new(Mutex::new(iter.into_iter().collect())))
    }

    fn add(&mut self, value: T) {
        self.0.try_lock().unwrap().push_back(value);
    }

    fn pop_front(&self) -> Option<T> {
        self.0.try_lock().unwrap().pop_front()
    }
}

#[derive(Clone)]
struct Stubs<T>(Arc<Mutex<Box<dyn Iterator<Item = T> + Send>>>);

impl<T: 'static + Send> Stubs<T> {
    pub fn next(&self) -> T {
        self.0
            .try_lock()
            .unwrap()
            .next()
            .expect("No more stub values!")
    }

    pub fn chain<I>(self, other: I) -> Self
    where
        I: IntoIterator<Item = T> + 'static,
        I::IntoIter: Send,
    {
        let old_iter = Arc::into_inner(self.0).unwrap().into_inner().unwrap();
        Self(Arc::new(Mutex::new(Box::new(old_iter.chain(other)))))
    }

    pub fn add(self, value: T) -> Self {
        self.chain(iter::once(value))
    }
}

impl<T: 'static + Send> Default for Stubs<T> {
    fn default() -> Self {
        Self(Arc::new(Mutex::new(Box::new(iter::empty()))))
    }
}

impl<T, I> From<I> for Stubs<T>
where
    T: 'static,
    I: IntoIterator<Item = T, IntoIter: Send> + 'static,
{
    fn from(stubs: I) -> Self {
        Self(Arc::new(Mutex::new(Box::new(stubs.into_iter()))))
    }
}
