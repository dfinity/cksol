use crate::ledger_init_args::ledger_init_args;
use candid::{CandidType, Encode, Nat, Principal, utils::ArgumentEncoder};
use canlog::{Log, LogEntry};
use cksol_types::{
    Address, DepositStatus, GetDepositAddressArgs, MinterInfo, RetrieveSolArgs, RetrieveSolError,
    RetrieveSolOk, RetrieveSolStatus, UpdateBalanceArgs, UpdateBalanceError,
};
use cksol_types_internal::{MinterArg, log::Priority};
use ic_canister_runtime::Runtime;
use ic_http_types::{HttpRequest, HttpResponse};
use ic_management_canister_types::{CanisterId, CanisterSettings};
use ic_pocket_canister_runtime::{ExecuteHttpOutcallMocks, PocketIcRuntime};
use icrc_ledger_types::icrc1::account::Account;
use num_traits::cast::ToPrimitive;
use pocket_ic::{PocketIcBuilder, RejectResponse, nonblocking::PocketIc};
use serde::de::DeserializeOwned;
use sol_rpc_client::SolRpcClient;
use sol_rpc_types::{Lamport, RpcAccess};
use std::{env::var, fs, path::PathBuf};

pub mod fixtures;
pub mod ledger_init_args;

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
    env: Option<PocketIc>,
    minter_canister_id: CanisterId,
    ledger_canister_id: CanisterId,
    caller: Option<Principal>,
}

impl Setup {
    pub const DEFAULT_CONTROLLER: Principal = Principal::from_slice(&[0x9d, 0xf7, 0x01]);
    pub const DEFAULT_CALLER: Principal = Principal::from_slice(&[0x9d, 0xf7, 0x02]);
    pub const DEFAULT_MINIMUM_WITHDRAWAL_AMOUNT: u64 = 10_000_000;
    pub const DEFAULT_DEPOSIT_FEE: Lamport = 50;

    async fn new(caller: Option<Principal>) -> Self {
        let env = PocketIcBuilder::new()
            .with_nns_subnet() //make_live requires NNS subnet.
            .with_fiduciary_subnet()
            .build_async()
            .await;

        let canister_settings = CanisterSettings {
            controllers: Some(vec![Self::DEFAULT_CONTROLLER]),
            ..CanisterSettings::default()
        };

        // Setup SOL RPC canister
        let sol_rpc_canister_id = env
            .create_canister_with_settings(None, Some(canister_settings.clone()))
            .await;
        env.add_cycles(sol_rpc_canister_id, u64::MAX as u128).await;
        env.install_canister(
            sol_rpc_canister_id,
            sol_rpc_wasm().await,
            Encode!(&sol_rpc_types::InstallArgs::default()).unwrap(),
            Some(Self::DEFAULT_CONTROLLER),
        )
        .await;
        Self::mock_sol_rpc_api_keys(&env, sol_rpc_canister_id).await;

        // Create ledger canister
        let ledger_canister_id = env
            .create_canister_with_settings(None, Some(canister_settings.clone()))
            .await;
        env.add_cycles(ledger_canister_id, u64::MAX as u128).await;

        // Setup ckSOL minter canister
        let minter_canister_id = env
            .create_canister_with_settings(None, Some(canister_settings.clone()))
            .await;
        env.add_cycles(minter_canister_id, u64::MAX as u128).await;
        env.install_canister(
            minter_canister_id,
            cksol_minter_wasm(),
            Encode!(&cksol_minter_init_args(
                sol_rpc_canister_id,
                ledger_canister_id
            ))
            .unwrap(),
            Some(Self::DEFAULT_CONTROLLER),
        )
        .await;

        // Install ledger canister
        env.install_canister(
            ledger_canister_id,
            ledger_wasm().await,
            Encode!(&ledger_init_args(minter_canister_id)).unwrap(),
            Some(Self::DEFAULT_CONTROLLER),
        )
        .await;

        Self {
            env: Some(env),
            minter_canister_id,
            ledger_canister_id,
            caller,
        }
    }

    async fn mock_sol_rpc_api_keys(env: &PocketIc, sol_rpc_canister_id: Principal) {
        const MOCK_API_KEY: &str = "mock-api-key";

        let runtime = PocketIcRuntime::new(env, Self::DEFAULT_CALLER);
        let client = SolRpcClient::builder(runtime, sol_rpc_canister_id).build();

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
            sol_rpc_canister_id,
            Self::DEFAULT_CONTROLLER,
            "updateApiKeys",
            Encode!(&api_keys).unwrap(),
        )
        .await
        .expect("BUG: Failed to call updateApiKeys");
    }

    pub fn runtime(&self) -> PocketIcRuntime<'_> {
        PocketIcRuntime::new(
            self.env.as_ref().unwrap(),
            self.caller.unwrap_or(Self::DEFAULT_CALLER),
        )
    }

    pub fn minter(&self) -> CkSolMinter<'_> {
        CkSolMinter(Canister {
            runtime: self.runtime(),
            id: self.minter_canister_id,
        })
    }

    pub fn ledger(&self) -> Ledger<'_> {
        Ledger(Canister {
            runtime: self.runtime(),
            id: self.ledger_canister_id,
        })
    }

    pub fn with_caller(mut self, caller: Principal) -> Self {
        self.caller = Some(caller);
        self
    }

    pub async fn drop(self) {
        let mut setup = self;
        if let Some(env) = setup.env.take() {
            env.drop().await
        }
    }
}

impl Drop for Setup {
    fn drop(&mut self) {
        if self.env.is_some() {
            panic!("Setup was not dropped properly. Call Setup::drop().await to clean up.");
        }
    }
}

pub struct CkSolMinter<'a>(Canister<'a>);

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
        self.0.try_update_call("get_deposit_address", (args,)).await
    }

    pub async fn update_balance(
        &self,
        args: UpdateBalanceArgs,
    ) -> Result<DepositStatus, UpdateBalanceError> {
        self.try_update_balance(args)
            .await
            .expect("update_balance failed")
    }

    pub async fn try_update_balance(
        &self,
        args: UpdateBalanceArgs,
    ) -> Result<Result<DepositStatus, UpdateBalanceError>, String> {
        self.0.try_update_call("update_balance", (args,)).await
    }

    pub async fn retrieve_sol(
        &self,
        args: RetrieveSolArgs,
    ) -> Result<RetrieveSolOk, RetrieveSolError> {
        self.0.update_call("retrieve_sol", (args,)).await
    }

    pub async fn retrieve_sol_status(&self, block_index: u64) -> RetrieveSolStatus {
        self.0
            .update_call("retrieve_sol_status", (block_index,))
            .await
    }

    pub async fn get_minter_info(&self) -> MinterInfo {
        self.0.query_call("get_minter_info", ()).await
    }

    pub async fn retrieve_logs(&self, priority: &Priority) -> Vec<LogEntry<Priority>> {
        let request = HttpRequest {
            method: "POST".to_string(),
            url: format!("/logs?priority={priority}"),
            headers: vec![],
            body: Default::default(),
        };
        let response: HttpResponse = self.0.query_call("http_request", (request,)).await;
        serde_json::from_slice::<Log<Priority>>(&response.body)
            .expect("failed to parse SOL RPC canister log")
            .entries
    }

    pub async fn upgrade(
        &self,
        upgrade_args: cksol_types_internal::UpgradeArgs,
    ) -> Result<(), RejectResponse> {
        self.0.upgrade(upgrade_args).await
    }

    pub fn with_http_mocks(mut self, mocks: impl ExecuteHttpOutcallMocks + 'static) -> Self {
        self.0 = self.0.with_http_mocks(mocks);
        self
    }
}

pub struct Ledger<'a>(Canister<'a>);

impl Ledger<'_> {
    pub async fn balance_of(&self, account: Account) -> u64 {
        self.0
            .update_call::<(Account,), Nat>("icrc1_balance_of", (account,))
            .await
            .0
            .to_u64()
            .unwrap()
    }

    pub async fn stop(&self) {
        self.0.stop().await;
    }
}

pub struct Canister<'a> {
    runtime: PocketIcRuntime<'a>,
    id: CanisterId,
}

impl Canister<'_> {
    async fn update_call<In, Out>(&self, method: &str, args: In) -> Out
    where
        In: ArgumentEncoder + Send,
        Out: CandidType + DeserializeOwned,
    {
        self.try_update_call(method, args)
            .await
            .unwrap_or_else(|e| panic!("Update call failed: {e}"))
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

    async fn query_call<In, Out>(&self, method: &str, args: In) -> Out
    where
        In: ArgumentEncoder + Send,
        Out: CandidType + DeserializeOwned,
    {
        self.try_query_call(method, args)
            .await
            .unwrap_or_else(|e| panic!("Query call failed: {e}"))
    }

    async fn try_query_call<In, Out>(&self, method: &str, args: In) -> Result<Out, String>
    where
        In: ArgumentEncoder + Send,
        Out: CandidType + DeserializeOwned,
    {
        self.runtime
            .query_call(self.id, method, args)
            .await
            .map_err(|e| format!("{:?}", e))
    }

    pub async fn upgrade(
        &self,
        upgrade_args: cksol_types_internal::UpgradeArgs,
    ) -> Result<(), RejectResponse> {
        self.runtime
            .as_ref()
            .upgrade_canister(
                self.id,
                cksol_minter_wasm(),
                Encode!(&MinterArg::Upgrade(upgrade_args)).unwrap(),
                Some(Setup::DEFAULT_CONTROLLER),
            )
            .await
    }

    pub async fn stop(&self) {
        self.runtime
            .as_ref()
            .stop_canister(self.id, Some(Setup::DEFAULT_CONTROLLER))
            .await
            .expect("Failed to stop canister");
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

fn cksol_minter_init_args(
    sol_rpc_canister_id: Principal,
    ledger_canister_id: Principal,
) -> MinterArg {
    use cksol_types_internal::{Ed25519KeyName, InitArgs, MinterArg};
    MinterArg::Init(InitArgs {
        sol_rpc_canister_id,
        ledger_canister_id,
        deposit_fee: Setup::DEFAULT_DEPOSIT_FEE,
        master_key_name: Ed25519KeyName::MainnetProdKey1,
        minimum_withdrawal_amount: Setup::DEFAULT_MINIMUM_WITHDRAWAL_AMOUNT,
    })
}

async fn sol_rpc_wasm() -> Vec<u8> {
    const DOWNLOAD_PATH: &str = "../wasms/sol_rpc_canister.wasm.gz";
    const DOWNLOAD_URL: &str = "https://github.com/dfinity/sol-rpc-canister/releases/latest/download/sol_rpc_canister.wasm.gz";
    canister_wasm(DOWNLOAD_PATH, DOWNLOAD_URL).await
}

async fn ledger_wasm() -> Vec<u8> {
    const DOWNLOAD_PATH: &str = "../wasms/ic-icrc1-ledger.wasm.gz";
    const DOWNLOAD_URL: &str = "https://github.com/dfinity/ic/releases/download/ledger-suite-icrc-2026-02-02/ic-icrc1-ledger.wasm.gz";
    canister_wasm(DOWNLOAD_PATH, DOWNLOAD_URL).await
}

async fn canister_wasm(download_path: &str, url: &str) -> Vec<u8> {
    let path = PathBuf::from(var("CARGO_MANIFEST_DIR").unwrap()).join(download_path);

    if let Ok(wasm) = fs::read(&path) {
        return wasm;
    }

    let bytes = reqwest::get(url)
        .await
        .unwrap_or_else(|e| panic!("Failed to fetch canister WASM: {e:?}"))
        .bytes()
        .await
        .unwrap_or_else(|e| panic!("Failed to read bytes from canister WASM: {e:?}"))
        .to_vec();

    let _ = fs::write(&path, &bytes);

    bytes
}
