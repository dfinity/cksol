use candid::CandidType;
use ic_canister_runtime::{IcError, IcRuntime, Runtime, StubRuntime};
use std::cell::RefCell;
use std::fmt::Debug;
use std::iter;

pub trait CanisterRuntime {
    fn inter_canister_call_runtime(&self) -> impl Runtime;
    fn time(&self) -> u64;
    fn instruction_counter(&self) -> u64;
}

#[derive(Default, Debug)]
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
}

pub struct TestCanisterRuntime {
    inter_canister_call_runtime: StubRuntime,
    times: RefCell<Box<dyn Iterator<Item = u64>>>,
    instruction_counts: RefCell<Box<dyn Iterator<Item = u64>>>,
}

impl Default for TestCanisterRuntime {
    fn default() -> Self {
        Self {
            inter_canister_call_runtime: Default::default(),
            times: RefCell::new(Box::new(iter::empty())),
            instruction_counts: RefCell::new(Box::new(iter::empty())),
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
        self.times = RefCell::new(Box::new(0..));
        self
    }

    pub fn with_stub_times(mut self, times: impl IntoIterator<Item = u64> + 'static) -> Self {
        self.times = RefCell::new(Box::new(times.into_iter()));
        self
    }
}

impl CanisterRuntime for TestCanisterRuntime {
    fn inter_canister_call_runtime(&self) -> impl Runtime {
        // This clone returns a new reference to the same stubs
        self.inter_canister_call_runtime.clone()
    }

    fn time(&self) -> u64 {
        self.times.borrow_mut().next().expect("No more stub times!")
    }

    fn instruction_counter(&self) -> u64 {
        self.instruction_counts
            .borrow_mut()
            .next()
            .expect("No more stub instruction counts!")
    }
}
