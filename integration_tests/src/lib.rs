use candid::{CandidType, Encode, Principal, utils::ArgumentEncoder};
use cksol_types::GetDepositAddressArgs;
use ic_canister_runtime::Runtime;
use ic_management_canister_types::{CanisterId, CanisterSettings};
use ic_pocket_canister_runtime::PocketIcRuntime;
use pocket_ic::{PocketIcBuilder, nonblocking::PocketIc};
use serde::de::DeserializeOwned;
use std::{env::var, path::PathBuf, sync::Arc};

#[derive(Default)]
pub struct SetupBuilder {}

impl SetupBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn build(self) -> Setup {
        Setup::new().await
    }
}

pub struct Setup {
    env: Arc<PocketIc>,
    minter_canister_id: CanisterId,
}

impl Setup {
    pub const DEFAULT_CONTROLLER: Principal = Principal::from_slice(&[0x9d, 0xf7, 0x01]);
    pub const DEFAULT_CALLER: Principal = Principal::from_slice(&[0x9d, 0xf7, 0x02]);

    pub async fn new() -> Self {
        let env = PocketIcBuilder::new()
            .with_nns_subnet() //make_live requires NNS subnet.
            .with_fiduciary_subnet()
            .build_async()
            .await;

        let canister_settings = CanisterSettings {
            controllers: Some(vec![Self::DEFAULT_CONTROLLER]),
            ..CanisterSettings::default()
        };

        // Setup ckSOL minter canister
        let minter_canister_id = env
            .create_canister_with_settings(None, Some(canister_settings.clone()))
            .await;
        env.add_cycles(minter_canister_id, u64::MAX as u128).await;
        env.install_canister(
            minter_canister_id,
            cksol_minter_wasm(),
            Encode!().unwrap(),
            Some(Self::DEFAULT_CONTROLLER),
        )
        .await;

        Self {
            env: Arc::new(env),
            minter_canister_id,
        }
    }

    pub fn runtime(&self) -> PocketIcRuntime<'_> {
        PocketIcRuntime::new(self.env.as_ref(), Self::DEFAULT_CALLER)
    }

    pub fn minter(&self) -> CkSolMinter<'_> {
        CkSolMinter {
            runtime: self.runtime(),
            id: self.minter_canister_id,
        }
    }
}

pub struct CkSolMinter<'a> {
    runtime: PocketIcRuntime<'a>,
    id: CanisterId,
}

impl CkSolMinter<'_> {
    pub async fn get_deposit_address(&self, args: GetDepositAddressArgs) -> sol_rpc_types::Pubkey {
        self.update_call("getDepositAddress", (args,)).await
    }

    async fn update_call<In, Out>(&self, method: &str, args: In) -> Out
    where
        In: ArgumentEncoder + Send,
        Out: CandidType + DeserializeOwned,
    {
        self.runtime
            .update_call(self.id, method, args, 0)
            .await
            .expect("Update call failed")
    }
}

fn cksol_minter_wasm() -> Vec<u8> {
    ic_test_utilities_load_wasm::load_wasm(
        PathBuf::from(var("CARGO_MANIFEST_DIR").unwrap()).join("../minter"),
        "cksol-minter",
        &[],
    )
}
