use candid::{CandidType, Encode, Principal, utils::ArgumentEncoder};
use cksol_types::{Address, GetDepositAddressArgs};
use ic_canister_runtime::Runtime;
use ic_management_canister_types::{CanisterId, CanisterSettings};
use ic_pocket_canister_runtime::PocketIcRuntime;
use pocket_ic::{PocketIcBuilder, nonblocking::PocketIc};
use serde::de::DeserializeOwned;
use std::{env::var, path::PathBuf, sync::Arc};

#[derive(Default)]
pub struct SetupBuilder {
    caller: Option<Principal>,
}

impl SetupBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_caller(mut self, caller: Principal) -> Self {
        self.caller = Some(caller);
        self
    }

    pub async fn build(self) -> Setup {
        Setup::new(self.caller).await
    }
}

pub struct Setup {
    env: Arc<PocketIc>,
    minter_canister_id: CanisterId,
    caller: Option<Principal>,
}

impl Setup {
    pub const DEFAULT_CONTROLLER: Principal = Principal::from_slice(&[0x9d, 0xf7, 0x01]);
    pub const DEFAULT_CALLER: Principal = Principal::from_slice(&[0x9d, 0xf7, 0x02]);

    pub async fn new(caller: Option<Principal>) -> Self {
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
            Encode!(&cksol_minter_init_args()).unwrap(),
            Some(Self::DEFAULT_CONTROLLER),
        )
        .await;

        Self {
            env: Arc::new(env),
            minter_canister_id,
            caller,
        }
    }

    pub fn runtime(&self) -> PocketIcRuntime<'_> {
        PocketIcRuntime::new(
            self.env.as_ref(),
            self.caller.unwrap_or(Self::DEFAULT_CALLER),
        )
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
    pub async fn get_deposit_address(&self, args: GetDepositAddressArgs) -> Address {
        self.try_get_deposit_address(args)
            .await
            .expect("get_deposit_address failed")
    }

    pub async fn try_get_deposit_address(
        &self,
        args: GetDepositAddressArgs,
    ) -> Result<Address, String> {
        self.try_update_call("get_deposit_address", (args,)).await
    }

    async fn try_update_call<In, Out>(&self, method: &str, args: In) -> Result<Out, String>
    where
        In: ArgumentEncoder + Send,
        Out: CandidType + DeserializeOwned,
    {
        self.runtime
            .update_call(self.id, method, args, 0)
            .await
            .map_err(|e| format!("{:?}", e))
    }
}

fn cksol_minter_wasm() -> Vec<u8> {
    ic_test_utilities_load_wasm::load_wasm(
        PathBuf::from(var("CARGO_MANIFEST_DIR").unwrap()).join("../minter"),
        "cksol-minter",
        &[],
    )
}

fn cksol_minter_init_args() -> cksol_types::lifecycle::MinterArg {
    use cksol_types::lifecycle;
    lifecycle::MinterArg::Init(lifecycle::InitArgs {
        // TODO DEFI-2643: Fix me!
        sol_rpc_canister_id: Principal::anonymous(),
        // TODO DEFI-2643: Fix me!
        ledger_canister_id: Principal::anonymous(),
        deposit_fee: 0,
        master_key_name: lifecycle::Ed25519KeyName::MainnetProdKey1,
    })
}
