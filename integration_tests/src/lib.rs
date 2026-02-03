use candid::{CandidType, Encode, Principal, encode_one, utils::ArgumentEncoder};
use cksol_types::{Address, GetDepositAddressArgs, UpdateBalanceArgs, UpdateBalanceError};
use ic_canister_runtime::Runtime;
use ic_management_canister_types::{CanisterId, CanisterSettings};
use ic_pocket_canister_runtime::{ExecuteHttpOutcallMocks, PocketIcRuntime};
use pocket_ic::{PocketIcBuilder, nonblocking::PocketIc};
use serde::de::DeserializeOwned;
use sol_rpc_client::{SOL_RPC_CANISTER, SolRpcClient};
use sol_rpc_types::{Lamport, RpcAccess};
use std::{env::var, fs, path::PathBuf, sync::Arc};

#[derive(Default)]
pub enum PocketIcMode {
    LiveMode,
    #[default]
    NonLiveMode,
}

#[derive(Default)]
pub struct SetupBuilder {
    caller: Option<Principal>,
    make_live: Option<PocketIcMode>,
    sol_rpc_install_args: Option<sol_rpc_types::InstallArgs>,
}

impl SetupBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_caller(mut self, caller: Principal) -> Self {
        self.caller = Some(caller);
        self
    }

    pub fn with_pocket_ic_live_mode(mut self) -> Self {
        self.make_live = Some(PocketIcMode::LiveMode);
        self
    }

    pub fn with_sol_rpc_install_args(mut self, args: sol_rpc_types::InstallArgs) -> Self {
        self.sol_rpc_install_args = Some(args);
        self
    }

    pub async fn build(self) -> Setup {
        Setup::new(
            self.caller,
            self.make_live.unwrap_or_default(),
            self.sol_rpc_install_args.unwrap_or_default(),
        )
        .await
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

    pub async fn new(
        caller: Option<Principal>,
        make_live: PocketIcMode,
        sol_rpc_install_args: sol_rpc_types::InstallArgs,
    ) -> Self {
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

        // Setup SOL RPC canister
        env.create_canister_with_id(None, Some(canister_settings), SOL_RPC_CANISTER)
            .await
            .unwrap_or_else(|e| {
                panic!("Could not create SOL RPC canister with ID {SOL_RPC_CANISTER:?}: {e:?}")
            });
        env.add_cycles(SOL_RPC_CANISTER, u64::MAX as u128).await;
        env.install_canister(
            SOL_RPC_CANISTER,
            sol_rpc_wasm().await,
            Encode!(&sol_rpc_install_args).unwrap(),
            Some(Self::DEFAULT_CONTROLLER),
        )
        .await;
        Self::mock_sol_rpc_api_keys(&env).await;

        let env = if let PocketIcMode::LiveMode = make_live {
            let mut env = env;
            let _endpoint = env.make_live(None).await;
            env
        } else {
            env
        };

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

    async fn mock_sol_rpc_api_keys(env: &PocketIc) {
        const MOCK_API_KEY: &str = "mock-api-key";

        let runtime = PocketIcRuntime::new(env, Self::DEFAULT_CALLER);
        let client = SolRpcClient::builder(runtime, SOL_RPC_CANISTER).build();

        let providers = client.get_providers().await;
        let mut api_keys = Vec::new();
        for (id, provider) in providers {
            match provider.access {
                RpcAccess::Authenticated { .. } => {
                    api_keys.push((id, Some(MOCK_API_KEY.to_string())));
                }
                RpcAccess::Unauthenticated { .. } => {}
            }
        }
        env.update_call(
            SOL_RPC_CANISTER,
            Self::DEFAULT_CONTROLLER,
            "updateApiKeys",
            encode_one(api_keys).unwrap(),
        )
        .await
        .expect("BUG: Failed to call updateApiKeys");
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

    pub async fn update_balance(
        &self,
        args: UpdateBalanceArgs,
    ) -> Result<Lamport, UpdateBalanceError> {
        self.update_call("updateBalance", (args,)).await
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

    pub fn with_http_mocks(mut self, mocks: impl ExecuteHttpOutcallMocks + 'static) -> Self {
        self.runtime = self.runtime.with_http_mocks(mocks);
        self
    }
}

fn cksol_minter_wasm() -> Vec<u8> {
    ic_test_utilities_load_wasm::load_wasm(
        PathBuf::from(var("CARGO_MANIFEST_DIR").unwrap()).join("../minter"),
        "cksol-minter",
        &[],
    )
}

async fn sol_rpc_wasm() -> Vec<u8> {
    let path =
        PathBuf::from(var("CARGO_MANIFEST_DIR").unwrap()).join("../wasms/sol_rpc_canister.wasm.gz");

    if let Ok(wasm) = fs::read(&path) {
        return wasm;
    }

    let bytes = reqwest::get("https://github.com/dfinity/sol-rpc-canister/releases/latest/download/sol_rpc_canister.wasm.gz")
        .await
        .unwrap_or_else(|e| panic!("Failed to fetch SOL RPC canister WASM: {e:?}"))
        .bytes()
        .await
        .unwrap_or_else(|e| panic!("Failed to read bytes from SOL RPC canister WASM: {e:?}"))
        .to_vec();

    let _ = fs::write(&path, &bytes);

    bytes
}
