use assert2::check;
use base64::{Engine, engine::general_purpose::STANDARD};
use candid::Principal;
use cksol_int_tests::{
    Setup, SetupBuilder,
    fixtures::{
        FINALIZE_TRANSACTIONS_DELAY, MINTER_ADDRESS, MockBuilder, PROCESS_PENDING_MINTS_DELAY,
        SUBMISSION_BLOCK_HEIGHT, SWEEP_DEPOSITS_DELAY,
    },
    validator::FEE_PER_SIGNATURE,
};
use cksol_types::{DepositSplArgs, DepositSplId, GetDepositAddressArgs, Lamport, Signature};
use cksol_types_internal::{
    SplTokenRegistration, UpgradeArgs,
    event::{EventType, TransactionPurpose},
};
use ic_pocket_canister_runtime::JsonRpcResponse;
use icrc_ledger_types::icrc1::account::Account;
use serde_json::json;
use sol_rpc_types::{AccountData, AccountEncoding, AccountInfo};
use solana_address::{Address, address};
use solana_program_pack::Pack;
use solana_transaction::Transaction;
use spl_associated_token_account_interface::address::get_associated_token_address;
use spl_token_interface::state::{Account as TokenAccount, AccountState};

pub const MINT: Address = address!("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v");
pub const OTHER_MINT: Address = address!("So11111111111111111111111111111111111111112");
pub const DECIMALS: u8 = 6;
pub const MINIMUM_DEPOSIT_AMOUNT: u64 = 1_000_000;
/// Fixed fee in the token's smallest units.
pub const DEPOSIT_FEE: u64 = 1;
pub const BALANCE_ABOVE_MINIMUM: u64 = MINIMUM_DEPOSIT_AMOUNT + DEPOSIT_FEE;
pub const RENT_EXEMPT_LAMPORTS: Lamport = 2_039_280;
/// The network fee of one SPL sweep: the signatures of the minter and of the deposit owner.
pub const SWEEP_FEE: Lamport = 2 * FEE_PER_SIGNATURE;

pub fn associated_token_address(owner: &Address) -> Address {
    get_associated_token_address(&owner.to_bytes().into(), &MINT.to_bytes().into())
        .to_bytes()
        .into()
}

pub fn account_info(data: Vec<u8>) -> AccountInfo {
    AccountInfo {
        lamports: RENT_EXEMPT_LAMPORTS,
        space: data.len() as u64,
        data: AccountData::Binary(STANDARD.encode(data), AccountEncoding::Base64),
        owner: spl_token_interface::id().to_string(),
        executable: false,
        rent_epoch: 0,
    }
}

pub fn token_account_data(owner: &Address) -> Vec<u8> {
    token_account(owner, BALANCE_ABOVE_MINIMUM, AccountState::Initialized)
}

pub fn token_account_data_with_amount(owner: &Address, amount: u64) -> Vec<u8> {
    token_account(owner, amount, AccountState::Initialized)
}

pub fn frozen_token_account_data(owner: &Address) -> Vec<u8> {
    token_account(owner, BALANCE_ABOVE_MINIMUM, AccountState::Frozen)
}

fn token_account(owner: &Address, amount: u64, state: AccountState) -> Vec<u8> {
    let mut data = vec![0; TokenAccount::LEN];
    TokenAccount::pack(
        TokenAccount {
            mint: MINT.to_bytes().into(),
            owner: owner.to_bytes().into(),
            amount,
            state,
            ..TokenAccount::default()
        },
        &mut data,
    )
    .unwrap();
    data
}

/// Funds the minter and its operating balance, installs a token ledger and registers
/// [`MINT`] on it. The funding uses RPC IDs 0 to 31.
pub async fn setup_with_registered_mint() -> Setup {
    let setup = SetupBuilder::new().with_proxy_canister().build().await;
    setup.fund_minter().await;
    setup.top_up_operating_balance().await;
    let ledger_id = setup.install_spl_ledger(DECIMALS).await.canister_id();
    register_token(&setup, spl_token_registration(ledger_id)).await;
    setup
}

pub fn spl_token_registration(ledger_id: Principal) -> SplTokenRegistration {
    SplTokenRegistration {
        mint: MINT.to_string(),
        token_program: spl_token_interface::id().to_string(),
        decimals: DECIMALS,
        ledger_id,
        index_id: None,
        symbol: "ckSPL".to_string(),
        minimum_deposit_amount: MINIMUM_DEPOSIT_AMOUNT,
        deposit_fee: DEPOSIT_FEE,
        deposits_paused: false,
        withdrawal_fee_token: None,
    }
}

/// Registers the token through a minter upgrade, which makes no HTTP outcalls.
pub async fn register_token(setup: &Setup, registration: SplTokenRegistration) {
    setup
        .minter()
        .upgrade(UpgradeArgs {
            add_spl_tokens: Some(vec![registration]),
            ..UpgradeArgs::default()
        })
        .await
        .expect("upgrade should register the token");
    // The init timer fetches the master key again.
    setup.tick().await;
}

/// Uses the submitted message, with an outcome where the given amount moved from the
/// source to the existing treasury ATA.
pub fn sweep_transaction_response(
    sweep: &Transaction,
    owner: &Address,
    amount: u64,
) -> JsonRpcResponse {
    sweep_outcome_response(sweep, owner, Some(amount))
}

/// Uses the submitted message, with an outcome where the transfer failed and only the
/// transaction fee was paid.
pub fn failed_sweep_transaction_response(sweep: &Transaction, owner: &Address) -> JsonRpcResponse {
    sweep_outcome_response(sweep, owner, None)
}

fn sweep_outcome_response(
    sweep: &Transaction,
    owner: &Address,
    moved: Option<u64>,
) -> JsonRpcResponse {
    let message = &sweep.message;
    let index = |address: &Address| {
        message
            .account_keys
            .iter()
            .position(|key| key == address)
            .unwrap()
    };
    let source_index = index(&associated_token_address(owner));
    let destination_index = index(&associated_token_address(&MINTER_ADDRESS));
    let fee = u64::from(message.header.num_required_signatures) * FEE_PER_SIGNATURE;
    let mut pre_balances = vec![1; message.account_keys.len()];
    pre_balances[0] = 5_000_000_000;
    pre_balances[source_index] = RENT_EXEMPT_LAMPORTS;
    pre_balances[destination_index] = RENT_EXEMPT_LAMPORTS;
    let mut post_balances = pre_balances.clone();
    post_balances[0] -= fee;
    let token_balance = |account_index: usize, owner: &Address, amount: u64| {
        json!({
            "accountIndex": account_index,
            "mint": MINT.to_string(),
            "owner": owner.to_string(),
            "programId": spl_token_interface::id().to_string(),
            "uiTokenAmount": {
                "amount": amount.to_string(),
                "decimals": DECIMALS,
                "uiAmount": null,
                "uiAmountString": solana_account_decoder_client_types::token::real_number_string_trimmed(amount, DECIMALS)
            }
        })
    };
    let source_balance = moved.unwrap_or(BALANCE_ABOVE_MINIMUM);
    let pre_token_balances = [
        token_balance(source_index, owner, source_balance),
        token_balance(destination_index, &MINTER_ADDRESS, 0),
    ];
    let (error, status, post_token_balances) = match moved {
        Some(amount) => (
            json!(null),
            json!({"Ok": null}),
            [
                token_balance(source_index, owner, 0),
                token_balance(destination_index, &MINTER_ADDRESS, amount),
            ],
        ),
        None => {
            let error = json!({ "InstructionError": [0, { "Custom": 17 }] });
            (
                error.clone(),
                json!({ "Err": error }),
                pre_token_balances.clone(),
            )
        }
    };
    // The recorded event has the first signature only; the owner signature is not checked.
    let mut sweep = sweep.clone();
    sweep.signatures.resize(
        usize::from(message.header.num_required_signatures),
        Default::default(),
    );
    let encoded = STANDARD.encode(bincode::serialize(&sweep).unwrap());
    JsonRpcResponse::from(json!({
        "jsonrpc": "2.0",
        "id": 1,
        "result": {
            "slot": 350_000_000_u64,
            "blockTime": 1700000000_i64,
            "transaction": [encoded, "base64"],
            "meta": {
                "err": error,
                "status": status,
                "fee": fee,
                "preBalances": pre_balances,
                "postBalances": post_balances,
                "preTokenBalances": pre_token_balances,
                "postTokenBalances": post_token_balances,
                "innerInstructions": [],
                "logMessages": [],
                "loadedAddresses": {"readonly": [], "writable": []},
                "rewards": []
            }
        }
    }))
}

/// The outcome that the mocked `getTransaction` reports for an SPL withdrawal.
pub enum WithdrawalOutcome {
    /// The transfer succeeded and created the destination token account.
    CreatedAccount,
    /// As [`Self::CreatedAccount`], with a rent of one lamport above the reserve.
    CreatedAccountAboveReserve,
    /// As [`Self::CreatedAccount`], with the given rent.
    CreatedAccountWithRent(Lamport),
    /// The transfer failed on-chain, so only the transaction fee was paid.
    Failed,
}

/// Uses the submitted message of an SPL withdrawal of `amount` tokens to `destination`,
/// from a treasury that holds [`BALANCE_ABOVE_MINIMUM`] tokens.
pub fn withdrawal_transaction_response(
    withdrawal: &Transaction,
    destination: &Address,
    amount: u64,
    outcome: WithdrawalOutcome,
) -> JsonRpcResponse {
    let message = &withdrawal.message;
    let index = |address: &Address| {
        message
            .account_keys
            .iter()
            .position(|key| key == address)
            .unwrap()
    };
    let treasury_index = index(&associated_token_address(&MINTER_ADDRESS));
    let destination_index = index(&associated_token_address(destination));
    let rent = match outcome {
        WithdrawalOutcome::CreatedAccount => RENT_EXEMPT_LAMPORTS,
        WithdrawalOutcome::CreatedAccountAboveReserve => RENT_EXEMPT_LAMPORTS + 1,
        WithdrawalOutcome::CreatedAccountWithRent(rent) => rent,
        WithdrawalOutcome::Failed => 0,
    };
    let mut pre_balances = vec![1; message.account_keys.len()];
    pre_balances[0] = 1_000_000_000_000;
    pre_balances[treasury_index] = RENT_EXEMPT_LAMPORTS;
    pre_balances[destination_index] = 0;
    let mut post_balances = pre_balances.clone();
    post_balances[0] -= FEE_PER_SIGNATURE + rent;
    post_balances[destination_index] += rent;
    let token_balance = |account_index: usize, owner: &Address, amount: u64| {
        json!({
            "accountIndex": account_index,
            "mint": MINT.to_string(),
            "owner": owner.to_string(),
            "programId": spl_token_interface::id().to_string(),
            "uiTokenAmount": {
                "amount": amount.to_string(),
                "decimals": DECIMALS,
                "uiAmount": null,
                "uiAmountString": solana_account_decoder_client_types::token::real_number_string_trimmed(amount, DECIMALS)
            }
        })
    };
    let pre_token_balances = vec![token_balance(
        treasury_index,
        &MINTER_ADDRESS,
        BALANCE_ABOVE_MINIMUM,
    )];
    let (error, status, post_token_balances) = match outcome {
        WithdrawalOutcome::CreatedAccount
        | WithdrawalOutcome::CreatedAccountAboveReserve
        | WithdrawalOutcome::CreatedAccountWithRent(_) => (
            json!(null),
            json!({"Ok": null}),
            vec![
                token_balance(
                    treasury_index,
                    &MINTER_ADDRESS,
                    BALANCE_ABOVE_MINIMUM - amount,
                ),
                token_balance(destination_index, destination, amount),
            ],
        ),
        WithdrawalOutcome::Failed => {
            // The token transfer is the third instruction. Its destination is frozen.
            let error = json!({ "InstructionError": [2, { "Custom": 17 }] });
            (
                error.clone(),
                json!({ "Err": error }),
                pre_token_balances.clone(),
            )
        }
    };
    let encoded = STANDARD.encode(bincode::serialize(withdrawal).unwrap());
    JsonRpcResponse::from(json!({
        "jsonrpc": "2.0",
        "id": 1,
        "result": {
            "slot": 350_000_000_u64,
            "blockTime": 1700000000_i64,
            "transaction": [encoded, "base64"],
            "meta": {
                "err": error,
                "status": status,
                "fee": FEE_PER_SIGNATURE,
                "preBalances": pre_balances,
                "postBalances": post_balances,
                "preTokenBalances": pre_token_balances,
                "postTokenBalances": post_token_balances,
                "innerInstructions": [],
                "logMessages": [],
                "loadedAddresses": {"readonly": [], "writable": []},
                "rewards": []
            }
        }
    }))
}

/// Registers the mint on a fresh SPL ledger, then queues a deposit from the caller's
/// associated token account and checks the recorded events.
pub async fn queue_deposit_for_caller() {
    let setup = SetupBuilder::new().with_proxy_canister().build().await;
    let ledger_id = setup.install_spl_ledger(DECIMALS).await.canister_id();
    let subaccount = Some([1; 32]);
    let owner: Address = setup
        .minter()
        .get_deposit_address(GetDepositAddressArgs {
            owner: None,
            subaccount,
        })
        .await
        .into();
    let source = associated_token_address(&owner);
    register_token(&setup, spl_token_registration(ledger_id)).await;

    let deposit_id = setup
        .minter()
        .with_http_mocks(
            MockBuilder::new()
                .get_account_info(&source, Some(account_info(token_account_data(&owner))))
                .build(),
        )
        .deposit_spl(DepositSplArgs {
            owner: None,
            subaccount,
            mint: MINT.into(),
        })
        .await
        .expect("deposit_spl should queue a sweep");

    assert_eq!(deposit_id, 0);
    setup.minter().assert_that_events().await.satisfy(|events| {
        check!(
            events.last()
                == Some(&EventType::QueuedSplDeposit {
                    deposit_id,
                    account: Account {
                        owner: setup.proxy_canister_id(),
                        subaccount,
                    },
                    mint: MINT.into(),
                    address: source.into(),
                    balance: BALANCE_ABOVE_MINIMUM,
                    fee: DEPOSIT_FEE,
                })
        );
    });

    setup.drop().await;
}

/// Queues a deposit of the full test balance for the account. The account read uses RPC
/// IDs `start_id` to `start_id + 3`.
pub async fn queue_deposit(setup: &Setup, account: Account, start_id: u64) -> DepositSplId {
    let owner: Address = setup.minter().get_deposit_address(account).await.into();
    setup
        .minter()
        .with_http_mocks(
            MockBuilder::with_start_id(start_id)
                .get_account_info(
                    &associated_token_address(&owner),
                    Some(account_info(token_account_data(&owner))),
                )
                .build(),
        )
        .deposit_spl(DepositSplArgs {
            owner: None,
            subaccount: account.subaccount,
            mint: MINT.into(),
        })
        .await
        .expect("deposit_spl should queue a sweep")
}

/// Runs the sweep timer and answers its getSlot, getBlock and sendTransaction calls with
/// RPC IDs `start_id` to `start_id + 11`. Returns the signature of the sweep.
pub async fn sweep_queued_deposit(
    setup: &Setup,
    deposit_id: DepositSplId,
    start_id: u64,
) -> Signature {
    setup.advance_time(SWEEP_DEPOSITS_DELAY).await;
    setup
        .execute_http_mocks(
            MockBuilder::with_start_id(start_id)
                .submit_transaction(SUBMISSION_BLOCK_HEIGHT)
                .build(),
        )
        .await;
    setup
        .minter()
        .get_all_events()
        .await
        .into_iter()
        .find_map(|event| match event.payload {
            EventType::SubmittedTransaction {
                signature,
                purpose: TransactionPurpose::SweepSplDeposit { deposit_id: id, .. },
                ..
            } if id == deposit_id => Some(signature),
            _ => None,
        })
        .expect("the SPL sweep timer should submit the queued deposit")
}

/// Runs the finalization timer and reports the sweep as finalized with the full token
/// transfer. The finalization calls use RPC IDs `start_id` to `start_id + 15`.
pub async fn finalize_sweep(setup: &Setup, signature: &Signature, owner: &Address, start_id: u64) {
    finalize_sweep_with_amount(setup, signature, owner, BALANCE_ABOVE_MINIMUM, start_id).await;
}

/// Like [`finalize_sweep`], with the given amount moved instead of the recorded balance.
pub async fn finalize_sweep_with_amount(
    setup: &Setup,
    signature: &Signature,
    owner: &Address,
    amount: u64,
    start_id: u64,
) {
    let sweep = setup.minter().signed_transaction(signature).await;
    setup.advance_time(FINALIZE_TRANSACTIONS_DELAY).await;
    setup
        .execute_http_mocks(
            MockBuilder::with_start_id(start_id)
                .finalize_transaction(signature, SUBMISSION_BLOCK_HEIGHT)
                .get_transaction(signature, sweep_transaction_response(&sweep, owner, amount))
                .build(),
        )
        .await;
}

/// Runs the mint timer and enough PocketIC rounds to complete the ledger calls.
pub async fn run_mint_timer(setup: &Setup) {
    setup
        .advance_time_and_settle(PROCESS_PENDING_MINTS_DELAY)
        .await;
}
