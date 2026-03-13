use ic_canister_runtime::{IcRuntime, Runtime};
use std::fmt::Debug;

pub trait CanisterRuntime {
    fn inter_canister_call_runtime(&self) -> impl Runtime;
    fn time(&self) -> u64;
    fn instruction_counter(&self) -> u64;
    fn msg_cycles_available(&self) -> u128;
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

    fn msg_cycles_available(&self) -> u128 {
        ic_cdk::api::msg_cycles_available()
    }
}
