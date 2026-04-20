use crate::signer::{IcSchnorrSigner, SchnorrSigner};
use candid::Principal;
use ic_canister_runtime::{IcRuntime, Runtime};
use ic_cdk_management_canister::{SchnorrPublicKeyArgs, SchnorrPublicKeyResult};
use std::{future::Future, time::Duration};

pub trait CanisterRuntime: Clone + 'static {
    fn inter_canister_call_runtime(&self) -> impl Runtime;
    fn signer(&self) -> impl SchnorrSigner;
    fn canister_self(&self) -> Principal;
    fn time(&self) -> u64;
    fn instruction_counter(&self) -> u64;
    fn msg_cycles_accept(&self, amount: u128) -> u128;
    fn msg_cycles_available(&self) -> u128;
    fn msg_cycles_refunded(&self) -> u128;
    fn set_timer<F, Fut>(&self, delay: Duration, f: F) -> ic_cdk_timers::TimerId
    where
        Self: Sized,
        F: FnOnce(Self) -> Fut + 'static,
        Fut: Future<Output = ()> + 'static;
    fn schnorr_public_key(
        &self,
        args: SchnorrPublicKeyArgs,
    ) -> impl Future<Output = SchnorrPublicKeyResult>;
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

    fn signer(&self) -> impl SchnorrSigner {
        IcSchnorrSigner
    }

    fn canister_self(&self) -> Principal {
        ic_cdk::api::canister_self()
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

    fn set_timer<F, Fut>(&self, delay: Duration, f: F) -> ic_cdk_timers::TimerId
    where
        Self: Sized,
        F: FnOnce(Self) -> Fut + 'static,
        Fut: Future<Output = ()> + 'static,
    {
        let runtime = self.clone();
        ic_cdk_timers::set_timer(delay, async move { f(runtime).await })
    }

    async fn schnorr_public_key(&self, args: SchnorrPublicKeyArgs) -> SchnorrPublicKeyResult {
        ic_cdk_management_canister::schnorr_public_key(&args)
            .await
            .expect("failed to obtain the Schnorr public key")
    }
}
