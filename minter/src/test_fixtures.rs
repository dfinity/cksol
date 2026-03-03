use crate::{
    state::{
        SchnorrPublicKey, State,
        event::{DepositEvent, Event, EventType, DepositId, MintedEvent},
        init_once_state, mutate_state,
    },
    storage::with_event_iter,
    numeric::LedgerMintIndex,
};
use candid::Principal;
use cksol_types::DepositStatus;
use cksol_types_internal::{Ed25519KeyName, InitArgs};
use ic_ed25519::{PocketIcMasterPublicKeyId, PublicKey};
use icrc_ledger_types::icrc1::account::Account;
use sol_rpc_types::Lamport;
use solana_address::{Address, address};
use solana_transaction_status_client_types::{
    EncodedConfirmedTransactionWithStatusMeta, EncodedTransaction,
    EncodedTransactionWithStatusMeta, TransactionBinaryEncoding, UiLoadedAddresses,
    UiTransactionStatusMeta, option_serializer::OptionSerializer,
};
use std::{collections::VecDeque, str::FromStr};

pub const BLOCK_INDEX: u64 = 98763_u64;
pub const DEPOSIT_FEE: Lamport = 10_000_000; // 0.01 SOL
pub const MINIMUM_WITHDRAWAL_AMOUNT: Lamport = 10_000_000; // 0.01 SOL
pub const MINIMUM_DEPOSIT_AMOUNT: Lamport = 10_000_000; // 0.01 SOL

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
        minimum_withdrawal_amount: MINIMUM_WITHDRAWAL_AMOUNT,
        minimum_deposit_amount: MINIMUM_DEPOSIT_AMOUNT,
    }
}

pub fn init_state() {
    init_state_with_args(valid_init_args());
}

pub fn init_state_with_args(init_args: InitArgs) {
    init_once_state(State::try_from(init_args).expect("Invalid init args"));
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
            any::<u64>(),
            any::<u64>(),
        )
            .prop_map(
                |(
                    sol_rpc_canister_id,
                    ledger_canister_id,
                    deposit_fee,
                    master_key_name,
                    minimum_withdrawal_amount,
                    minimum_deposit_amount,
                )| {
                    InitArgs {
                        sol_rpc_canister_id,
                        ledger_canister_id,
                        deposit_fee,
                        master_key_name,
                        minimum_withdrawal_amount,
                        minimum_deposit_amount,
                    }
                },
            )
    }

    pub fn arb_upgrade_args() -> impl Strategy<Value = UpgradeArgs> {
        (
            prop::option::of(arb_principal()),
            prop::option::of(any::<u64>()),
            prop::option::of(any::<u64>()),
            prop::option::of(any::<u64>()),
        )
            .prop_map(
                |(
                    sol_rpc_canister_id,
                    deposit_fee,
                    minimum_withdrawal_amount,
                    minimum_deposit_amount,
                )| UpgradeArgs {
                    sol_rpc_canister_id,
                    deposit_fee,
                    minimum_withdrawal_amount,
                    minimum_deposit_amount,
                },
            )
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

    pub const DEPOSIT_AMOUNT: Lamport = 500_000_000;
    pub const DEPOSIT_ADDRESS: Address = address!("BVH7GZXRdqyZLSLBS4cm1Yom8Yvekw6ytgSFz9y9on4e");
    pub const DEPOSITOR_PRINCIPAL: Principal = Principal::from_slice(&[0x9d, 0xf7, 0x02]);
    pub const DEPOSITOR_ACCOUNT: Account = Account {
        owner: DEPOSITOR_PRINCIPAL,
        subaccount: None,
    };

    pub fn deposit_status_processing() -> DepositStatus {
        DepositStatus::Processing(deposit_transaction_signature().into())
    }

    pub fn deposit_status_minted() -> DepositStatus {
        DepositStatus::Minted {
            block_index: BLOCK_INDEX,
            minted_amount: DEPOSIT_AMOUNT - DEPOSIT_FEE,
            signature: deposit_transaction_signature().into(),
        }
    }

    pub fn minted_event(mint_block_index: impl Into<LedgerMintIndex>) -> MintedEvent {
        MintedEvent {
            deposit_event: accepted_deposit_event(),
            minted_amount: DEPOSIT_AMOUNT - DEPOSIT_FEE,
            mint_block_index: mint_block_index.into(),
        }
    }

    pub fn accepted_deposit_event() -> DepositEvent {
        DepositEvent {
            deposit_id: DepositId {
                signature: deposit_transaction_signature(),
                account: DEPOSITOR_ACCOUNT,
            },
            deposit_amount: DEPOSIT_AMOUNT,
            amount_to_mint: DEPOSIT_AMOUNT - DEPOSIT_FEE,
        }
    }

    // https://explorer.solana.com/tx/49aFRmEtgnVN3UetkKHJbz3ZMcDY6pgS9oDoN4Y4NQYfHSx4nsDsx3PSKubxfmY69URcosJj3CWu4aypeddduZYX?cluster=devnet
    pub fn deposit_transaction_signature() -> solana_signature::Signature {
        const SIGNATURE: &str = "49aFRmEtgnVN3UetkKHJbz3ZMcDY6pgS9oDoN4Y4NQYfHSx4nsDsx3PSKubxfmY69URcosJj3CWu4aypeddduZYX";
        solana_signature::Signature::from_str(SIGNATURE).unwrap()
    }

    // 0.5 SOL transfer to DEPOSITOR_ACCOUNT's deposit address (BVH7GZXRdqyZLSLBS4cm1Yom8Yvekw6ytgSFz9y9on4e)
    // https://explorer.solana.com/tx/49aFRmEtgnVN3UetkKHJbz3ZMcDY6pgS9oDoN4Y4NQYfHSx4nsDsx3PSKubxfmY69URcosJj3CWu4aypeddduZYX?cluster=devnet
    pub fn deposit_transaction() -> EncodedConfirmedTransactionWithStatusMeta {
        const ENCODED_DEPOSIT_TRANSACTION: &str = "AZ1xufshIEi/hzGnwqjbgjUqDzcH3dfZQs3hZUbR8iHESSc+4eGeOwll0PMlDtORri5YQi433FjgQ5YK138CXQQBAAEDIg5JU11WGypQAKfOpxcE0+UIiKney1G6hf+6GRXcmseb01hqfWVQEn6n64lX4Uby5n5lTlmSpsWgEH1gv7LbVwAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA/S7SHgiiNOkFs7RGKc0VhLBrkHbCp47AK4FytcYYlDgBAgIAAQwCAAAAAGXNHQAAAAA=";
        EncodedConfirmedTransactionWithStatusMeta {
            slot: 443421331,
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
                    post_balances: vec![895811440, 500000000, 1],
                    post_token_balances: OptionSerializer::Some(vec![]),
                    pre_balances: vec![1395816440, 0, 1],
                    pre_token_balances: OptionSerializer::Some(vec![]),
                    rewards: OptionSerializer::None,
                    status: Ok(()),
                    return_data: OptionSerializer::Skip,
                }),
                version: None,
            },
            block_time: Some(1771582425),
        }
    }

    // https://explorer.solana.com/tx/3wuW2SB8BzrMZSL1KNuibQ17NKTAjS565mnMvt86smJXaMq99mPsD9QpCRXSfNRziXaxwrt9k1wDE1WFahPv4GgA?cluster=devnet
    pub fn deposit_transaction_to_wrong_address_signature() -> solana_signature::Signature {
        const SIGNATURE: &str = "3wuW2SB8BzrMZSL1KNuibQ17NKTAjS565mnMvt86smJXaMq99mPsD9QpCRXSfNRziXaxwrt9k1wDE1WFahPv4GgA";
        solana_signature::Signature::from_str(SIGNATURE).unwrap()
    }

    // 0.5 SOL transfer to 6sCCyJVCPgzu6VEgeqJyxhW9X2W6ijAAReCRTfD5iecH
    // https://explorer.solana.com/tx/3wuW2SB8BzrMZSL1KNuibQ17NKTAjS565mnMvt86smJXaMq99mPsD9QpCRXSfNRziXaxwrt9k1wDE1WFahPv4GgA?cluster=devnet
    pub fn deposit_transaction_to_wrong_address() -> EncodedConfirmedTransactionWithStatusMeta {
        const ENCODED_DEPOSIT_TRANSACTION: &str = "AZNh0+eJqGMu6d/1B6we8EPvCIQzZRV+VwGmaUsRncA9vy9LpqYzvs7XCzDZFvqUf0nmZPbLJxNsf/+MtMKdyQMBAAEDIg5JU11WGypQAKfOpxcE0+UIiKney1G6hf+6GRXcmsdXJiVs5okiCEmlhqTw1NKb4zDN/LDw/Yn6SZn3ERUu2gAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAfYVT4I2211RPtd7dum+9C2LuW1CxTsXdP5SBBrw5HE4BAgIAAQwCAAAAAGXNHQAAAAA=";
        EncodedConfirmedTransactionWithStatusMeta {
            slot: 443004539,
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
                    post_balances: vec![2895831440, 500000000, 1],
                    post_token_balances: OptionSerializer::Some(vec![]),
                    pre_balances: vec![3395836440, 0, 1],
                    pre_token_balances: OptionSerializer::Some(vec![]),
                    rewards: OptionSerializer::None,
                    status: Ok(()),
                    return_data: OptionSerializer::None,
                }),
                version: None,
            },
            block_time: Some(1771421240),
        }
    }

    // https://explorer.solana.com/tx/56LyqGhjJV4epkZbn9Q1bW1Qf6L5jP1oF7rRkSt9zWtDPpxdyBVxc73NfQxADBhdXjshGQi8WQJokGWjT9Z8z97v?cluster=devnet
    pub fn deposit_transaction_to_multiple_accounts_signature() -> solana_signature::Signature {
        const SIGNATURE: &str = "56LyqGhjJV4epkZbn9Q1bW1Qf6L5jP1oF7rRkSt9zWtDPpxdyBVxc73NfQxADBhdXjshGQi8WQJokGWjT9Z8z97v";
        solana_signature::Signature::from_str(SIGNATURE).unwrap()
    }

    // Single transaction that transfers funds to multiple accounts:
    //  - 0.1 SOL to BVH7GZXRdqyZLSLBS4cm1Yom8Yvekw6ytgSFz9y9on4e
    //  - 0.2 SOL to 36nNQ1JxjZ9tSN8WWqGPjV9H3FexvsMC5gEnkmUhigpY
    //  - 0.3 SOL to 75H1btFeRrFySZuKyZGPpvYcy3uDkcMoj5EL2mpsFUvr
    // https://explorer.solana.com/tx/56LyqGhjJV4epkZbn9Q1bW1Qf6L5jP1oF7rRkSt9zWtDPpxdyBVxc73NfQxADBhdXjshGQi8WQJokGWjT9Z8z97v?cluster=devnet
    pub fn deposit_transaction_to_multiple_accounts() -> EncodedConfirmedTransactionWithStatusMeta {
        const ENCODED_DEPOSIT_TRANSACTION: &str = "AcytR2Rq+c0hM6m/Fka99Q4d7R4Nin2Ic4z/c1DLSmPLkhiLffSIvYlQLLKH/zvcy3JgP/umG5TN9TLv9oSUYAkBAAEFIg5JU11WGypQAKfOpxcE0+UIiKney1G6hf+6GRXcmscfMpOhqUYjXIxXvJp/bhOwZFCsImXzz5iVqw/g+bBPiVo+jDsfe97gI2/mJd+TXE7nJj+D6zIOZsV4YmKTgeUvm9NYan1lUBJ+p+uJV+FG8uZ+ZU5ZkqbFoBB9YL+y21cAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAN9p0fDGOCOG2Vh6Cbo7MPuOUKoG2zX1iTCguRzb3oKRAwQCAAMMAgAAAADh9QUAAAAABAIAAQwCAAAAAMLrCwAAAAAEAgACDAIAAAAAo+ERAAAAAA==";
        EncodedConfirmedTransactionWithStatusMeta {
            slot: 445682829,
            transaction: EncodedTransactionWithStatusMeta {
                transaction: EncodedTransaction::Binary(
                    ENCODED_DEPOSIT_TRANSACTION.to_string(),
                    TransactionBinaryEncoding::Base64,
                ),
                meta: Some(UiTransactionStatusMeta {
                    compute_units_consumed: OptionSerializer::Some(450),
                    cost_units: OptionSerializer::Some(2387),
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
                        "Program 11111111111111111111111111111111 invoke [1]".to_string(),
                        "Program 11111111111111111111111111111111 success".to_string(),
                        "Program 11111111111111111111111111111111 invoke [1]".to_string(),
                        "Program 11111111111111111111111111111111 success".to_string(),
                    ]),
                    post_balances: vec![4295796440, 200000000, 300000000, 600000000, 1],
                    post_token_balances: OptionSerializer::Some(vec![]),
                    pre_balances: vec![4895801440, 0, 0, 500000000, 1],
                    pre_token_balances: OptionSerializer::Some(vec![]),
                    rewards: OptionSerializer::None,
                    status: Ok(()),
                    return_data: OptionSerializer::Skip,
                }),
                version: None,
            },
            block_time: Some(1772447561),
        }
    }
}

pub struct EventsAssert(VecDeque<Event>);

impl EventsAssert {
    pub fn from_recorded() -> Self {
        Self(with_event_iter(|events| events.collect()))
    }

    pub fn assert_no_events_recorded() {
        Self::from_recorded().assert_no_more_events();
    }

    pub fn expect_event<F>(mut self, check: F) -> Self
    where
        F: Fn(EventType),
    {
        let event = self.0.pop_front().expect("No more events!");
        check(event.payload);
        self
    }

    pub fn expect_event_eq(mut self, expected: EventType) -> Self {
        let event = self.0.pop_front().expect("No more events!");
        assert_eq!(event.payload, expected);
        self
    }

    pub fn assert_no_more_events(&self) {
        assert!(self.0.is_empty());
    }
}
