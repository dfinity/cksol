use async_trait::async_trait;
use cksol_types::{Signature, UpdateBalanceArgs};
use ic_pocket_canister_runtime::{ExecuteHttpOutcallMocks, MockHttpOutcalls};
use pocket_ic::nonblocking::PocketIc;
use std::{str::FromStr, sync::Arc};
use tokio::sync::Mutex;

pub const DEPOSIT_TRANSACTION_SIGNATURE: &str =
    "5nAMoTjRdRw4ah4WS7FPipqn3HYqZz9FMTLheVmN6mnJjgqFfComsZeAgBa6FBbX3bf5TNMegPjPE3PYQPCHup2s";

pub fn default_update_balance_args() -> UpdateBalanceArgs {
    UpdateBalanceArgs {
        owner: None,
        subaccount: None,
        signature: deposit_transaction_signature(),
    }
}

pub fn deposit_transaction_signature() -> Signature {
    Signature::from_str(DEPOSIT_TRANSACTION_SIGNATURE).unwrap()
}

/// This wrapper around [`MockHttpOutcalls`] allows different instances of [`PocketIcRuntime`]
/// to share the same mocks. This is useful in tests where several requests are made concurrently,
/// but only one of them results in HTTP outcalls being executed.
///
/// [`PocketIcRuntime`]: ic_pocket_canister_runtime::PocketIcRuntime
#[derive(Clone)]
pub struct SharedMockHttpOutcalls(Arc<Mutex<MockHttpOutcalls>>);

impl SharedMockHttpOutcalls {
    pub fn new(mocks: MockHttpOutcalls) -> Self {
        Self(Arc::new(Mutex::new(mocks)))
    }
}

#[async_trait]
impl ExecuteHttpOutcallMocks for SharedMockHttpOutcalls {
    async fn execute_http_outcall_mocks(&mut self, runtime: &PocketIc) -> () {
        self.0
            .lock()
            .await
            .execute_http_outcall_mocks(runtime)
            .await
    }
}
