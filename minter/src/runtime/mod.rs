use ic_canister_runtime::{IcRuntime, Runtime};
use std::{fmt::Debug, time::Duration};

pub trait CanisterRuntime: Clone + 'static {
    fn inter_canister_call_runtime(&self) -> impl Runtime;
    fn time(&self) -> u64;
    fn instruction_counter(&self) -> u64;
    fn msg_cycles_accept(&self, amount: u128) -> u128;
    fn msg_cycles_available(&self) -> u128;
    fn msg_cycles_refunded(&self) -> u128;
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

    fn msg_cycles_accept(&self, amount: u128) -> u128 {
        ic_cdk::api::msg_cycles_accept(amount)
    }

    fn msg_cycles_available(&self) -> u128 {
        ic_cdk::api::msg_cycles_available()
    }

    fn msg_cycles_refunded(&self) -> u128 {
        ic_cdk::api::msg_cycles_refunded()
    }

    fn set_timer(
        &self,
        delay: Duration,
        future: impl Future<Output = ()> + 'static,
    ) -> ic_cdk_timers::TimerId {
        ic_cdk_timers::set_timer(delay, future)
    }
}
