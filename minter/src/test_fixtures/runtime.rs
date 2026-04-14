use super::{signer::MockSchnorrSigner, stubs::Stubs};
use crate::{runtime::CanisterRuntime, signer::SchnorrSigner};
use candid::{CandidType, Principal};
use ic_canister_runtime::{IcError, Runtime, StubRuntime};
use ic_cdk_management_canister::SignCallError;
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
}
