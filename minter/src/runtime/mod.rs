use candid::CandidType;
use ic_canister_runtime::{IcError, IcRuntime, Runtime, StubRuntime};
use std::time::Duration;
use std::{
    fmt::Debug,
    iter,
    sync::{Arc, Mutex},
};

pub trait CanisterRuntime {
    fn inter_canister_call_runtime(&self) -> impl Runtime;
    fn time(&self) -> u64;
    fn instruction_counter(&self) -> u64;
    fn set_timer(
        &self,
        delay: Duration,
        future: impl Future<Output = ()> + 'static,
    ) -> ic_cdk_timers::TimerId;
}

#[derive(Clone, Default, Debug)]
pub struct IcCanisterRuntime(IcRuntime);

impl IcCanisterRuntime {
    pub fn new() -> Self {
        Self::default()
    }
}

impl CanisterRuntime for IcCanisterRuntime {
    fn inter_canister_call_runtime(&self) -> impl Runtime {
        self.0
    }

    fn time(&self) -> u64 {
        ic_cdk::api::time()
    }

    fn instruction_counter(&self) -> u64 {
        ic_cdk::api::instruction_counter()
    }

    fn set_timer(
        &self,
        delay: Duration,
        future: impl Future<Output = ()> + 'static,
    ) -> ic_cdk_timers::TimerId {
        ic_cdk_timers::set_timer(delay, future)
    }
}

#[derive(Clone)]
pub struct TestCanisterRuntime {
    inter_canister_call_runtime: StubRuntime,
    times: Arc<Mutex<dyn Iterator<Item = u64> + Send + Sync>>,
    instruction_counts: Arc<Mutex<dyn Iterator<Item = u64> + Send + Sync>>,
}

impl Default for TestCanisterRuntime {
    fn default() -> Self {
        Self {
            inter_canister_call_runtime: Default::default(),
            times: Arc::new(Mutex::new(iter::empty())),
            instruction_counts: Arc::new(Mutex::new(iter::empty())),
        }
    }
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
        self.times = Arc::new(Mutex::new(0..));
        self
    }

    pub fn with_stub_times<I>(mut self, times: I) -> Self
    where
        I: IntoIterator<Item = u64> + 'static,
        <I as IntoIterator>::IntoIter: Send + Sync,
    {
        self.times = Arc::new(Mutex::new(times.into_iter()));
        self
    }
}

impl CanisterRuntime for TestCanisterRuntime {
    fn inter_canister_call_runtime(&self) -> impl Runtime {
        // This clone returns a new reference to the same stubs
        self.inter_canister_call_runtime.clone()
    }

    fn time(&self) -> u64 {
        self.times
            .try_lock()
            .unwrap()
            .next()
            .expect("No more stub times!")
    }

    fn instruction_counter(&self) -> u64 {
        self.instruction_counts
            .try_lock()
            .unwrap()
            .next()
            .expect("No more stub instruction counts!")
    }

    fn set_timer(
        &self,
        _delay: Duration,
        _future: impl Future<Output = ()> + 'static,
    ) -> ic_cdk_timers::TimerId {
        Default::default()
    }
}
