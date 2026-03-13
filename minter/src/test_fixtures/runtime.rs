use crate::runtime::CanisterRuntime;
use candid::CandidType;
use ic_canister_runtime::{IcError, Runtime, StubRuntime};
use std::{
    iter,
    sync::{Arc, Mutex},
};

#[derive(Clone, Default)]
pub struct TestCanisterRuntime {
    inter_canister_call_runtime: StubRuntime,
    times: Stubs<u64>,
    instruction_counts: Stubs<u64>,
    msg_cycles_available: Stubs<u128>,
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

    pub fn add_msg_cycles_available(mut self, value: u128) -> Self {
        self.msg_cycles_available = self.msg_cycles_available.add(value);
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

    fn msg_cycles_available(&self) -> u128 {
        self.msg_cycles_available.next()
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
