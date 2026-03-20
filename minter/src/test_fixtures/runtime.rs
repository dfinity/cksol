use super::{signer::MockSchnorrSigner, stubs::Stubs};
use crate::{runtime::CanisterRuntime, signer::SchnorrSigner};
use candid::{CandidType, Principal};
use ic_canister_runtime::{IcError, Runtime, StubRuntime};
use ic_cdk::management_canister::SignCallError;
use std::time::Duration;

#[derive(Clone, Default)]
pub struct TestCanisterRuntime {
    inter_canister_call_runtime: StubRuntime,
    signer: MockSchnorrSigner,
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
        self.schnorr_signer = self.schnorr_signer.add_signature(signature);
        self
    }

    pub fn add_schnorr_signing_error(mut self, error: SignCallError) -> Self {
        self.schnorr_signer = self.schnorr_signer.add_response(Err(error));
        self
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
