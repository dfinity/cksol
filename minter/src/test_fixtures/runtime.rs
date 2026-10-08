use super::{
    confirmed_block, ledger_canister_id,
    signer::{MockSchnorrSigner, SignerExpectation, sign_for},
    sol_rpc_canister_id,
    stubs::Stubs,
};
use crate::{
    constants::{
        GET_ACCOUNT_INFO_CYCLES, GET_BALANCE_CYCLES, GET_RECENT_BLOCK_MAX_TRIES,
        GET_SIGNATURE_STATUSES_CYCLES, GET_TRANSACTION_CYCLES, MAX_HTTP_OUTCALL_RESPONSE_BYTES,
    },
    runtime::CanisterRuntime,
    signer::SchnorrSigner,
    test_fixtures::GetTransactionResult,
};
use async_trait::async_trait;
use base64::{Engine, engine::general_purpose::STANDARD};
use candid::{
    CandidType, Decode, Encode, Principal,
    utils::{ArgumentDecoder, ArgumentEncoder, decode_args, encode_args},
};
use ic_canister_runtime::{IcError, Runtime};
use ic_cdk::call::{CallPerformFailed, Error as CallError};
use ic_cdk_management_canister::{SchnorrPublicKeyArgs, SchnorrPublicKeyResult};
use icrc_ledger_types::{
    icrc1::{
        account::Account,
        transfer::{BlockIndex, TransferArg, TransferError},
    },
    icrc2::transfer_from::{TransferFromArgs, TransferFromError},
};
use mockall::{mock, predicate::eq};
use serde::de::DeserializeOwned;
use sol_rpc_types::{
    AccountInfo, CommitmentLevel, ConfirmedBlock, ConsensusStrategy, GetAccountInfoEncoding,
    GetAccountInfoParams, GetBalanceParams, GetBlockParams, GetSignatureStatusesParams,
    GetSlotParams, GetSlotRpcConfig, GetTransactionEncoding, GetTransactionParams, Lamport,
    MultiRpcResult, RpcConfig, RpcResult, RpcSources, SendTransactionParams, Slot, SolanaCluster,
    TransactionDetails, TransactionStatus,
};
use solana_address::Address;
use solana_transaction::Transaction;
use std::{
    fmt,
    future::Future,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::task::yield_now;

pub const TEST_CANISTER_ID: Principal = Principal::from_slice(&[0xCA; 10]);

/// The cycles `sol_rpc_client` attaches to a request whose cycles the minter leaves at the
/// default: `getSlot`, `getBlock` without transaction details or rewards, `sendTransaction`.
const SOL_RPC_DEFAULT_REQUEST_CYCLES: u128 = 10_000_000_000;

/// A [`CanisterRuntime`] whose inter-canister calls are backed by a mockall mock: every call
/// a test expects is registered upfront with one of the `expect_*` builder methods, stating
/// the canister, the method, the arguments, the attached cycles and the response. Each
/// expectation answers exactly one matching call; a call without a matching expectation and
/// an expectation that goes unused both fail the test.
#[derive(Clone, Default)]
pub struct TestCanisterRuntime {
    inter_canister_calls: Arc<Mutex<MockInterCanisterCalls>>,
    signer: MockSchnorrSigner,
    times: Stubs<u64>,
    expects_charges: bool,
    msg_cycles_accepted: Arc<Mutex<Vec<u128>>>,
    msg_cycles_available: Stubs<u128>,
    msg_cycles_refunded: Stubs<u128>,
    set_timer_delays: Arc<Mutex<Vec<Duration>>>,
    schnorr_public_key_results: Stubs<Result<SchnorrPublicKeyResult, CallError>>,
    schnorr_public_key_call_count: Arc<Mutex<usize>>,
}

impl TestCanisterRuntime {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers the mocks for one transaction paid for by `fee_payer` and submitted under
    /// `transaction_signature`. See [`TransactionBuilder`].
    pub fn transaction_builder(
        self,
        fee_payer: Account,
        transaction_signature: solana_signature::Signature,
    ) -> TransactionBuilder {
        TransactionBuilder::new(self, fee_payer, transaction_signature)
    }

    /// Expects one `getSlot` and, if `result` is a slot, one `getBlock` call for it, answered
    /// with [`confirmed_block`]. An error is answered to every retried `getSlot` call.
    pub fn add_recent_block(self, result: RpcResult<Slot>) -> Self {
        match result {
            Ok(slot) => self.expect_recent_block(slot, confirmed_block()),
            Err(error) => self.expect(
                get_slot_call(),
                CallResponse::from(MultiRpcResult::<Slot>::Consistent(Err(error))),
                GET_RECENT_BLOCK_MAX_TRIES.get(),
            ),
        }
    }

    /// Expects one `getSlot` call answered with `slot` and one `getBlock` call for `slot`
    /// answered with `block`.
    pub fn expect_recent_block(self, slot: Slot, block: ConfirmedBlock) -> Self {
        self.expect_get_slot(MultiRpcResult::Consistent(Ok(slot)))
            .expect_get_block(slot, MultiRpcResult::Consistent(Ok(Some(block))))
    }

    pub fn expect_get_slot(self, response: impl Into<CallResponse<MultiRpcResult<Slot>>>) -> Self {
        self.expect_once(get_slot_call(), response.into())
    }

    pub fn expect_get_block(
        self,
        slot: Slot,
        response: impl Into<CallResponse<MultiRpcResult<Option<ConfirmedBlock>>>>,
    ) -> Self {
        self.expect_once(get_block_call(slot), response.into())
    }

    pub fn expect_get_balance(
        self,
        address: Address,
        response: impl Into<CallResponse<MultiRpcResult<Lamport>>>,
    ) -> Self {
        self.expect_once(get_balance_call(address), response.into())
    }

    pub fn expect_get_transaction(
        self,
        signature: solana_signature::Signature,
        response: impl Into<CallResponse<GetTransactionResult>>,
    ) -> Self {
        self.expect_once(get_transaction_call(signature), response.into())
    }

    pub fn expect_get_signature_statuses(
        self,
        signatures: Vec<solana_signature::Signature>,
        response: impl Into<CallResponse<MultiRpcResult<Vec<Option<TransactionStatus>>>>>,
    ) -> Self {
        self.expect_once(get_signature_statuses_call(&signatures), response.into())
    }

    pub fn expect_get_account_info(
        self,
        address: Address,
        response: impl Into<CallResponse<MultiRpcResult<Option<AccountInfo>>>>,
    ) -> Self {
        self.expect_once(get_account_info_call(address), response.into())
    }

    /// Expects one `sendTransaction` call with the providers' preflight simulation whose
    /// transaction carries `transaction_signature` as its first signature, i.e. was signed by
    /// the fee payer with it.
    pub fn expect_send_transaction(
        self,
        transaction_signature: solana_signature::Signature,
        response: impl Into<CallResponse<MultiRpcResult<sol_rpc_types::Signature>>>,
    ) -> Self {
        self.expect_send_transaction_signed_with(transaction_signature, None, response.into())
    }

    /// Expects one `sendTransaction` call skipping the providers' preflight simulation whose
    /// transaction carries `transaction_signature` as its first signature.
    pub fn expect_send_transaction_skipping_preflight(
        self,
        transaction_signature: solana_signature::Signature,
        response: impl Into<CallResponse<MultiRpcResult<sol_rpc_types::Signature>>>,
    ) -> Self {
        self.expect_send_transaction_signed_with(transaction_signature, Some(true), response.into())
    }

    /// Expects one `sendTransaction` call skipping the providers' preflight simulation that
    /// sends exactly `transaction`.
    pub fn expect_send_exact_transaction_skipping_preflight(
        self,
        transaction: Transaction,
        response: impl Into<CallResponse<MultiRpcResult<sol_rpc_types::Signature>>>,
    ) -> Self {
        let mut params = SendTransactionParams::try_from(transaction)
            .expect("BUG: failed to serialize the expected transaction");
        params.skip_preflight = Some(true);
        self.expect_once(
            sol_rpc_call(
                "sendTransaction",
                (rpc_sources(), Some(rpc_config()), params),
                SOL_RPC_DEFAULT_REQUEST_CYCLES,
            ),
            response.into(),
        )
    }

    fn expect_send_transaction_signed_with(
        self,
        transaction_signature: solana_signature::Signature,
        skip_preflight: Option<bool>,
        response: CallResponse<MultiRpcResult<sol_rpc_types::Signature>>,
    ) -> Self {
        let response = response.encode();
        self.inter_canister_calls
            .lock()
            .unwrap()
            .expect_update_call()
            .withf(move |id, method, args, cycles| {
                *id == sol_rpc_canister_id()
                    && method == "sendTransaction"
                    && *cycles == SOL_RPC_DEFAULT_REQUEST_CYCLES
                    && sent_transaction(args).is_some_and(|params| {
                        params.skip_preflight == skip_preflight
                            && first_signature(&params) == Some(transaction_signature)
                    })
            })
            .times(1)
            .return_once(move |_, _, _, _| response);
        self
    }

    pub fn expect_icrc1_transfer(
        self,
        args: TransferArg,
        response: impl Into<CallResponse<Result<BlockIndex, TransferError>>>,
    ) -> Self {
        self.expect_once(
            UpdateCall {
                canister_id: ledger_canister_id(),
                method: "icrc1_transfer",
                args: CandidArgs::of((args,)),
                cycles: 0,
            },
            response.into(),
        )
    }

    pub fn expect_icrc2_transfer_from(
        self,
        args: TransferFromArgs,
        response: impl Into<CallResponse<Result<BlockIndex, TransferFromError>>>,
    ) -> Self {
        self.expect_once(
            UpdateCall {
                canister_id: ledger_canister_id(),
                method: "icrc2_transfer_from",
                args: CandidArgs::of((args,)),
                cycles: 0,
            },
            response.into(),
        )
    }

    pub fn add_times<I>(mut self, times: I) -> Self
    where
        I: IntoIterator<Item = u64>,
    {
        for time in times {
            self.times = self.times.add(time);
        }
        self
    }

    pub fn with_increasing_time(self) -> Self {
        self.with_increasing_time_from(0)
    }

    pub fn with_increasing_time_from(mut self, start: u64) -> Self {
        self.times = (start..).into();
        self
    }

    pub fn expecting_charges(mut self) -> Self {
        self.expects_charges = true;
        self
    }

    pub fn msg_cycles_accepted(&self) -> Vec<u128> {
        self.msg_cycles_accepted.lock().unwrap().clone()
    }

    pub fn add_msg_cycles_available(mut self, value: u128) -> Self {
        self.msg_cycles_available = self.msg_cycles_available.add(value);
        self
    }

    pub fn add_msg_cycles_refunded(mut self, value: u128) -> Self {
        self.msg_cycles_refunded = self.msg_cycles_refunded.add(value);
        self
    }

    pub fn add_signer(mut self, expectation: SignerExpectation) -> Self {
        self.signer = self.signer.add_signer(expectation);
        self
    }

    pub fn with_schnorr_public_key(mut self, result: SchnorrPublicKeyResult) -> Self {
        self.schnorr_public_key_results = self.schnorr_public_key_results.add(Ok(result));
        self
    }

    pub fn with_schnorr_public_key_call_failure(mut self) -> Self {
        self.schnorr_public_key_results = self
            .schnorr_public_key_results
            .add(Err(CallPerformFailed.into()));
        self
    }

    pub(crate) fn set_timer_call_count(&self) -> usize {
        self.set_timer_delays().len()
    }

    pub(crate) fn set_timer_delays(&self) -> Vec<Duration> {
        self.set_timer_delays.lock().unwrap().clone()
    }

    pub(crate) fn schnorr_public_key_call_count(&self) -> usize {
        *self.schnorr_public_key_call_count.lock().unwrap()
    }

    fn expect_once<Out: CandidType>(self, call: UpdateCall, response: CallResponse<Out>) -> Self {
        self.expect(call, response, 1)
    }

    fn expect<Out: CandidType>(
        self,
        call: UpdateCall,
        response: CallResponse<Out>,
        times: usize,
    ) -> Self {
        let response = response.encode();
        self.inter_canister_calls
            .lock()
            .unwrap()
            .expect_update_call()
            .with(
                eq(call.canister_id),
                eq(call.method.to_string()),
                eq(call.args),
                eq(call.cycles),
            )
            .times(times)
            .returning(move |_, _, _, _| response.clone());
        self
    }
}

impl CanisterRuntime for TestCanisterRuntime {
    fn inter_canister_call_runtime(&self) -> impl Runtime {
        MockRuntime(self.inter_canister_calls.clone())
    }

    fn signer(&self) -> impl SchnorrSigner {
        self.signer.clone()
    }

    fn canister_self(&self) -> Principal {
        TEST_CANISTER_ID
    }

    fn time(&self) -> u64 {
        self.times.next()
    }

    fn instruction_counter(&self) -> u64 {
        unimplemented!("TestCanisterRuntime does not model the instruction counter")
    }

    fn msg_cycles_accept(&self, amount: u128) -> u128 {
        assert!(
            self.expects_charges,
            "a test that does not expect the caller to be charged must not accept cycles, \
             but {amount} cycles were accepted: call \
             TestCanisterRuntime::expecting_charges() to opt in to being charged"
        );
        self.msg_cycles_accepted.lock().unwrap().push(amount);
        amount
    }

    fn msg_cycles_available(&self) -> u128 {
        self.msg_cycles_available.next()
    }

    fn msg_cycles_refunded(&self) -> u128 {
        self.msg_cycles_refunded.next()
    }

    fn set_timer<F, Fut>(&self, delay: Duration, _f: F) -> ic_cdk_timers::TimerId
    where
        Self: Sized,
        F: FnOnce(Self) -> Fut + 'static,
        Fut: Future<Output = ()> + 'static,
    {
        self.set_timer_delays.lock().unwrap().push(delay);
        Default::default()
    }

    async fn schnorr_public_key(
        &self,
        _args: SchnorrPublicKeyArgs,
    ) -> Result<SchnorrPublicKeyResult, CallError> {
        *self.schnorr_public_key_call_count.lock().unwrap() += 1;
        suspend_like_an_inter_canister_call().await;
        self.schnorr_public_key_results.next()
    }
}

/// Suspends the caller once, so that concurrent callers all reach the call before any of
/// them sees its response, as they do on the IC.
async fn suspend_like_an_inter_canister_call() {
    yield_now().await;
}

/// How the mock answers one expected inter-canister call: with a Candid reply or by failing
/// the call itself. Both the reply type and [`IcError`] convert into it, so an expectation
/// takes either directly.
pub enum CallResponse<Out> {
    Reply(Out),
    Failed(IcError),
}

impl<Out: CandidType> CallResponse<Out> {
    fn encode(self) -> Result<Vec<u8>, IcError> {
        match self {
            Self::Reply(reply) => {
                Ok(Encode!(&reply).expect("BUG: failed to encode Candid response"))
            }
            Self::Failed(error) => Err(error),
        }
    }
}

impl<T: CandidType> From<MultiRpcResult<T>> for CallResponse<MultiRpcResult<T>> {
    fn from(result: MultiRpcResult<T>) -> Self {
        Self::Reply(result)
    }
}

impl<T, E> From<Result<T, E>> for CallResponse<Result<T, E>> {
    fn from(result: Result<T, E>) -> Self {
        Self::Reply(result)
    }
}

impl<Out> From<IcError> for CallResponse<Out> {
    fn from(error: IcError) -> Self {
        Self::Failed(error)
    }
}

mock! {
    InterCanisterCalls {
        fn update_call(
            &self,
            id: Principal,
            method: String,
            args: CandidArgs,
            cycles: u128,
        ) -> Result<Vec<u8>, IcError>;

        fn query_call(
            &self,
            id: Principal,
            method: String,
            args: CandidArgs,
        ) -> Result<Vec<u8>, IcError>;
    }
}

/// The [`Runtime`] handed to the code under test: it erases each call's generic arguments
/// into their Candid encoding, relays the call to the shared [`MockInterCanisterCalls`], and
/// decodes the expectation's response.
struct MockRuntime(Arc<Mutex<MockInterCanisterCalls>>);

#[async_trait]
impl Runtime for MockRuntime {
    async fn update_call<In, Out>(
        &self,
        id: Principal,
        method: &str,
        args: In,
        cycles: u128,
    ) -> Result<Out, IcError>
    where
        In: ArgumentEncoder + Send,
        Out: CandidType + DeserializeOwned,
    {
        let response = self.0.lock().unwrap().update_call(
            id,
            method.to_string(),
            CandidArgs::of(args),
            cycles,
        );
        suspend_like_an_inter_canister_call().await;
        Ok(Decode!(&response?, Out).expect("BUG: failed to decode Candid response"))
    }

    async fn query_call<In, Out>(
        &self,
        id: Principal,
        method: &str,
        args: In,
    ) -> Result<Out, IcError>
    where
        In: ArgumentEncoder + Send,
        Out: CandidType + DeserializeOwned,
    {
        let response =
            self.0
                .lock()
                .unwrap()
                .query_call(id, method.to_string(), CandidArgs::of(args));
        suspend_like_an_inter_canister_call().await;
        Ok(Decode!(&response?, Out).expect("BUG: failed to decode Candid response"))
    }
}

/// The Candid-encoded arguments of an inter-canister call, compared by encoding and printed
/// as Candid text in mockall's failure messages.
#[derive(Clone, PartialEq, Eq)]
pub struct CandidArgs(Vec<u8>);

impl CandidArgs {
    fn of<In: ArgumentEncoder>(args: In) -> Self {
        Self(encode_args(args).expect("BUG: failed to encode Candid arguments"))
    }

    fn decode<Args>(&self) -> Result<Args, candid::Error>
    where
        Args: for<'a> ArgumentDecoder<'a>,
    {
        decode_args(&self.0)
    }
}

impl fmt::Debug for CandidArgs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match candid_parser::IDLArgs::from_bytes(&self.0) {
            Ok(args) => write!(f, "{args}"),
            Err(_) => write!(f, "{:?}", self.0),
        }
    }
}

struct UpdateCall {
    canister_id: Principal,
    method: &'static str,
    args: CandidArgs,
    cycles: u128,
}

fn sol_rpc_call<In: ArgumentEncoder>(method: &'static str, args: In, cycles: u128) -> UpdateCall {
    UpdateCall {
        canister_id: sol_rpc_canister_id(),
        method,
        args: CandidArgs::of(args),
        cycles,
    }
}

/// The [`RpcSources`] the minter's SOL RPC client sends with every call under
/// [`super::valid_init_args`].
fn rpc_sources() -> RpcSources {
    RpcSources::Default(SolanaCluster::Mainnet)
}

/// The [`RpcConfig`] the minter's SOL RPC client sends with every call.
fn rpc_config() -> RpcConfig {
    RpcConfig {
        response_consensus: Some(ConsensusStrategy::Threshold {
            min: 3,
            total: Some(4),
        }),
        ..RpcConfig::default()
    }
}

fn with_response_size_estimate(config: RpcConfig) -> RpcConfig {
    RpcConfig {
        response_size_estimate: Some(MAX_HTTP_OUTCALL_RESPONSE_BYTES),
        ..config
    }
}

fn get_slot_call() -> UpdateCall {
    sol_rpc_call(
        "getSlot",
        (
            rpc_sources(),
            Some(GetSlotRpcConfig::from(rpc_config())),
            None::<GetSlotParams>,
        ),
        SOL_RPC_DEFAULT_REQUEST_CYCLES,
    )
}

fn get_block_call(slot: Slot) -> UpdateCall {
    sol_rpc_call(
        "getBlock",
        (
            rpc_sources(),
            Some(rpc_config()),
            GetBlockParams {
                slot,
                commitment: None,
                max_supported_transaction_version: Some(0),
                transaction_details: Some(TransactionDetails::None),
                rewards: Some(false),
            },
        ),
        SOL_RPC_DEFAULT_REQUEST_CYCLES,
    )
}

fn get_balance_call(address: Address) -> UpdateCall {
    sol_rpc_call(
        "getBalance",
        (
            rpc_sources(),
            Some(rpc_config()),
            GetBalanceParams {
                commitment: Some(CommitmentLevel::Finalized),
                ..address.into()
            },
        ),
        GET_BALANCE_CYCLES,
    )
}

fn get_transaction_call(signature: solana_signature::Signature) -> UpdateCall {
    sol_rpc_call(
        "getTransaction",
        (
            rpc_sources(),
            Some(with_response_size_estimate(rpc_config())),
            GetTransactionParams {
                commitment: Some(CommitmentLevel::Finalized),
                max_supported_transaction_version: Some(0),
                encoding: Some(GetTransactionEncoding::Base64),
                ..signature.into()
            },
        ),
        GET_TRANSACTION_CYCLES,
    )
}

fn get_signature_statuses_call(signatures: &[solana_signature::Signature]) -> UpdateCall {
    let params = GetSignatureStatusesParams {
        search_transaction_history: Some(true),
        ..GetSignatureStatusesParams::try_from(signatures.iter().collect::<Vec<_>>())
            .expect("BUG: too many signatures for one getSignatureStatuses call")
    };
    sol_rpc_call(
        "getSignatureStatuses",
        (
            rpc_sources(),
            Some(with_response_size_estimate(rpc_config())),
            params,
        ),
        GET_SIGNATURE_STATUSES_CYCLES,
    )
}

fn get_account_info_call(address: Address) -> UpdateCall {
    sol_rpc_call(
        "getAccountInfo",
        (
            rpc_sources(),
            Some(rpc_config()),
            GetAccountInfoParams {
                commitment: Some(CommitmentLevel::Finalized),
                encoding: Some(GetAccountInfoEncoding::Base64),
                ..GetAccountInfoParams::from_pubkey(address)
            },
        ),
        GET_ACCOUNT_INFO_CYCLES,
    )
}

/// The parameters of the given `sendTransaction` arguments, if they are sent with the
/// minter's RPC sources and configuration.
fn sent_transaction(args: &CandidArgs) -> Option<SendTransactionParams> {
    let (sources, config, params): (RpcSources, Option<RpcConfig>, SendTransactionParams) =
        args.decode().ok()?;
    (sources == rpc_sources() && config == Some(rpc_config())).then_some(params)
}

/// The first signature of the sent transaction, i.e. the fee payer's signature identifying
/// the submitted transaction.
fn first_signature(params: &SendTransactionParams) -> Option<solana_signature::Signature> {
    let transaction = STANDARD.decode(params.get_transaction()).ok()?;
    let transaction: Transaction = bincode::deserialize(&transaction).ok()?;
    transaction.signatures.first().copied()
}

/// Expects the fee payer to sign with the transaction signature, answers the
/// `sendTransaction` call for the transaction carrying it with that same signature, and
/// expects every account added with [`Self::add_signers`] to sign with its own derived
/// signature, which the test reads back with `account_signature`.
///
/// Chain a further [`TestCanisterRuntime::transaction_builder`] onto [`Self::build`] for
/// each additional transaction a test expects.
pub struct TransactionBuilder(TestCanisterRuntime);

impl TransactionBuilder {
    fn new(
        runtime: TestCanisterRuntime,
        fee_payer: Account,
        transaction_signature: solana_signature::Signature,
    ) -> Self {
        Self(
            runtime
                .add_signer(sign_for(&fee_payer).expect([Ok(transaction_signature)]))
                .expect_send_transaction(
                    transaction_signature,
                    MultiRpcResult::Consistent(Ok(transaction_signature.into())),
                ),
        )
    }

    pub fn add_signers(mut self, accounts: impl IntoIterator<Item = Account>) -> Self {
        for account in accounts {
            self.0 = self.0.add_signer(sign_for(&account));
        }
        self
    }

    pub fn build(self) -> TestCanisterRuntime {
        self.0
    }
}
