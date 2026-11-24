//! Synthetic single-transfer outcomes with real signatures over the planned message.
use super::{account, devnet_sweep};
use crate::{
    address::{account_address, associated_token_address},
    state::{QueuedSplDeposit, SplSweep, SupportedSplToken},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use candid::Principal;
use cksol_types_internal::SplTokenRegistration;
use solana_address::Address;
use solana_hash::Hash;
use solana_transaction::{Transaction, versioned::VersionedTransaction};
use solana_transaction_status_client_types::{
    EncodedConfirmedTransactionWithStatusMeta, UiTransactionStatusMeta,
};

pub fn mint() -> Address {
    [2; 32].into()
}

pub fn registration() -> SplTokenRegistration {
    SplTokenRegistration {
        mint: mint().to_string(),
        token_program: spl_token_interface::id().to_string(),
        decimals: 6,
        ledger_id: Principal::from_slice(&[99, 1]),
        index_id: None,
        symbol: "ckTest".into(),
        minimum_deposit_amount: 1,
        deposit_fee: 10,
        deposits_paused: false,
    }
}

pub fn plan(id: u64) -> SplSweep {
    let token = SupportedSplToken {
        mint: mint(),
        token_program: spl_token_interface::id().to_bytes().into(),
        decimals: 6,
        ledger_id: registration().ledger_id,
        index_id: None,
        symbol: "ckTest".into(),
        minimum_deposit_amount: 1,
        deposit_fee: 10,
        deposits_paused: false,
    };
    let account = account(id as usize + 1);
    let owner = account_address(&devnet_sweep::master_key(), &account);
    SplSweep::plan(
        id,
        QueuedSplDeposit {
            account,
            mint: mint(),
            address: associated_token_address(&owner, &mint()),
            balance: 1_000 + id,
            fee: 10,
        },
        &token,
        &devnet_sweep::master_key(),
    )
    .unwrap()
}

pub fn transaction(sweep: &SplSweep) -> VersionedTransaction {
    let message = sweep.sweep_message(Hash::new_from_array([0x42; 32]));
    let signatures = vec![
        devnet_sweep::minter_signature_of(&message),
        devnet_sweep::account_signature_of(&sweep.deposit().account, &message),
    ];
    Transaction {
        message,
        signatures,
    }
    .into()
}

pub fn meta(sweep: &SplSweep) -> UiTransactionStatusMeta {
    let transaction = transaction(sweep);
    let keys = transaction.message.static_account_keys();
    let main = devnet_sweep::minter_main_address();
    let owner = account_address(&devnet_sweep::master_key(), &sweep.deposit().account);
    let treasury = associated_token_address(&main, &mint());
    let source_index = keys
        .iter()
        .position(|key| *key == sweep.deposit().address)
        .unwrap();
    let treasury_index = keys.iter().position(|key| *key == treasury).unwrap();
    let token_balance = |index, owner: Address, amount: u64| {
        serde_json::json!({
            "accountIndex": index, "mint": mint().to_string(), "owner": owner.to_string(),
            "programId": spl_token_interface::id().to_string(),
            "uiTokenAmount": {"amount": amount.to_string(), "decimals": 6, "uiAmount": null, "uiAmountString": "0"}
        })
    };
    let mut pre = vec![2_000_000u64; keys.len()];
    pre[0] = 10_000_000;
    let mut post = pre.clone();
    post[0] -= 10_000;
    serde_json::from_value(serde_json::json!({
        "err": null, "status": {"Ok": null}, "fee": 10_000, "preBalances": pre, "postBalances": post,
        "preTokenBalances": [token_balance(source_index, owner, sweep.deposit().balance + 100), token_balance(treasury_index, main, 500)],
        "postTokenBalances": [token_balance(source_index, owner, 100), token_balance(treasury_index, main, 500 + sweep.deposit().balance)]
    })).unwrap()
}

pub fn outcome(
    sweep: &SplSweep,
    meta: Option<UiTransactionStatusMeta>,
) -> EncodedConfirmedTransactionWithStatusMeta {
    serde_json::from_value(serde_json::json!({
        "slot": 300_000_000u64, "blockTime": null,
        "transaction": [STANDARD.encode(bincode::serialize(&transaction(sweep)).unwrap()), "base64"],
        "meta": meta,
    })).unwrap()
}

pub fn init_balance(amount: u64) {
    devnet_sweep::init_master_key();
    super::fund_minter(
        devnet_sweep::fresh_deposit(account(0xFD), amount),
        devnet_sweep::minter_main_address(),
    );
}
