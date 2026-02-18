use candid::Principal;
use cksol_types_internal::{Ed25519KeyName, InitArgs};

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
        prop::option::of(any::<u64>()).prop_map(|deposit_fee| UpgradeArgs { deposit_fee })
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
