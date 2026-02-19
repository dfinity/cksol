use crate::state::{SchnorrPublicKey, State, init_once_state, mutate_state};
use candid::Principal;
use cksol_types_internal::{Ed25519KeyName, InitArgs};
use ic_ed25519::{PocketIcMasterPublicKeyId, PublicKey};
use icrc_ledger_types::icrc1::account::Account;
use solana_transaction_status_client_types::{
    EncodedConfirmedTransactionWithStatusMeta, EncodedTransaction,
    EncodedTransactionWithStatusMeta, TransactionBinaryEncoding, UiLoadedAddresses,
    UiTransactionStatusMeta, option_serializer::OptionSerializer,
};
use std::str::FromStr;

pub const DEPOSIT_FEE: u64 = 50;

pub fn sol_rpc_canister_id() -> Principal {
    Principal::from_slice(&[1_u8; 20])
}

pub fn ledger_canister_id() -> Principal {
    Principal::from_slice(&[2_u8; 20])
}

pub fn valid_init_args() -> InitArgs {
    InitArgs {
        sol_rpc_canister_id: sol_rpc_canister_id(),
        ledger_canister_id: ledger_canister_id(),
        deposit_fee: DEPOSIT_FEE,
        master_key_name: Ed25519KeyName::default(),
    }
}

pub fn init_state() {
    init_once_state(State::try_from(valid_init_args()).expect("Invalid init args"));
}

pub fn init_schnorr_master_key() {
    mutate_state(|s| {
        s.set_once_minter_public_key(SchnorrPublicKey {
            public_key: PublicKey::pocketic_key(PocketIcMasterPublicKeyId::Key1),
            chain_code: [1; 32],
        })
    });
}

pub mod arb {
    use crate::state::event::{Event, EventType};
    use cksol_types_internal::{Ed25519KeyName, InitArgs, UpgradeArgs};
    use proptest::prelude::{Just, Strategy, any, prop, prop_oneof};

    pub fn arb_principal() -> impl Strategy<Value = candid::Principal> {
        prop::collection::vec(any::<u8>(), 0..=29)
            .prop_map(|bytes| candid::Principal::from_slice(&bytes))
    }

    pub fn arb_ed25519_key_name() -> impl Strategy<Value = Ed25519KeyName> {
        prop_oneof![
            Just(Ed25519KeyName::LocalDevelopment),
            Just(Ed25519KeyName::MainnetTestKey1),
            Just(Ed25519KeyName::MainnetProdKey1),
        ]
    }

    pub fn arb_init_args() -> impl Strategy<Value = InitArgs> {
        (
            arb_principal(),
            arb_principal(),
            any::<u64>(),
            arb_ed25519_key_name(),
        )
            .prop_map(
                |(sol_rpc_canister_id, ledger_canister_id, deposit_fee, master_key_name)| {
                    InitArgs {
                        sol_rpc_canister_id,
                        ledger_canister_id,
                        deposit_fee,
                        master_key_name,
                    }
                },
            )
    }

    pub fn arb_upgrade_args() -> impl Strategy<Value = UpgradeArgs> {
        (
            prop::option::of(arb_principal()),
            prop::option::of(any::<u64>()),
        )
            .prop_map(|(sol_rpc_canister_id, deposit_fee)| UpgradeArgs {
                sol_rpc_canister_id,
                deposit_fee,
            })
    }

    pub fn arb_event_type() -> impl Strategy<Value = EventType> {
        prop_oneof![
            arb_init_args().prop_map(EventType::Init),
            arb_upgrade_args().prop_map(EventType::Upgrade),
        ]
    }

    pub fn arb_event() -> impl Strategy<Value = Event> {
        (any::<u64>(), arb_event_type())
            .prop_map(|(timestamp, payload)| Event { timestamp, payload })
    }
}

pub mod deposit {
    use super::*;

    pub const DEPOSITOR_PRINCIPAL: Principal = Principal::from_slice(&[0x9d, 0xf7, 0x02]);
    pub const DEPOSITOR_ACCOUNT: Account = Account {
        owner: DEPOSITOR_PRINCIPAL,
        subaccount: None,
    };

    pub fn deposit_transaction_signature() -> solana_signature::Signature {
        const SIGNATURE: &str = "41MZzSM5aXRFBbPdaFyqueRPhp6VJbFHESvfKRvhXXnqB5hkhDpyRqdAPE8mTgbpfUPxP7bjhQK7JdUuykKtk2Xh";
        solana_signature::Signature::from_str(SIGNATURE).unwrap()
    }

    // Transfer from Solana address 3HwVowmCYKPWjRvkqfEfYFWetZLPmZW6LCnLEQDHqpJJ to
    // BQc4UB4yuhHRT5r6jyQFnUi54W5ZoXW8Lvfd6VaKoQfc on the Solana Devnet
    pub fn deposit_transaction() -> EncodedConfirmedTransactionWithStatusMeta {
        const ENCODED_DEPOSIT_TRANSACTION: &str = "AZZbWHQKwAkndrT0gmTPUn6tfnTAFYqJE8HTh+0OQ1f4dX1l/ah54VdJ/O9j1jNSZorH8+2BalrdbeONiWyxuwABAAEDIg5JU11WGypQAKfOpxcE0+UIiKney1G6hf+6GRXcmseaoN6/9tbZrK9zoPY+wNeEqI5eps8+kDCZ3zXX9UB+awAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAXfitZAvU6Mv/pQabeVthGg5LZFYn4GS4UMLpNalqt+4BAgIAAQwCAAAAAGXNHQAAAAA=";
        EncodedConfirmedTransactionWithStatusMeta {
            slot: 443005390,
            transaction: EncodedTransactionWithStatusMeta {
                transaction: EncodedTransaction::Binary(
                    ENCODED_DEPOSIT_TRANSACTION.to_string(),
                    TransactionBinaryEncoding::Base64,
                ),
                meta: Some(UiTransactionStatusMeta {
                    compute_units_consumed: OptionSerializer::Some(150),
                    cost_units: OptionSerializer::Some(1481),
                    err: None,
                    fee: 5000,
                    inner_instructions: OptionSerializer::Some(vec![]),
                    loaded_addresses: OptionSerializer::Some(UiLoadedAddresses {
                        writable: vec![],
                        readonly: vec![],
                    }),
                    log_messages: OptionSerializer::Some(vec![
                        "Program 11111111111111111111111111111111 invoke [1]".to_string(),
                        "Program 11111111111111111111111111111111 success".to_string(),
                    ]),
                    post_balances: vec![1895821440, 500000000, 1],
                    post_token_balances: OptionSerializer::Some(vec![]),
                    pre_balances: vec![2395826440, 0, 1],
                    pre_token_balances: OptionSerializer::Some(vec![]),
                    rewards: OptionSerializer::None,
                    status: Ok(()),
                    return_data: OptionSerializer::Skip,
                }),
                version: None,
            },
            block_time: Some(1771421567),
        }
    }
}
