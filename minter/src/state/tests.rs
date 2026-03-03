use super::{event::*, *};
use crate::{
    runtime::TestCanisterRuntime,
    state::audit::process_event,
    test_fixtures::{
        DEPOSIT_FEE, MINIMUM_DEPOSIT_AMOUNT, MINIMUM_WITHDRAWAL_AMOUNT, arb::arb_event,
        ledger_canister_id, sol_rpc_canister_id, valid_init_args,
    },
};
use assert_matches::assert_matches;
use cksol_types_internal::{Ed25519KeyName, InitArgs, UpgradeArgs};
use ic_stable_structures::Storable;
use proptest::prelude::*;
use std::borrow::Cow;

proptest! {
    #[test]
    fn event_minicbor_roundtrip(event in arb_event()) {
        let bytes = event.to_bytes();
        let decoded = Event::from_bytes(Cow::Borrowed(&bytes));
        assert_eq!(event, decoded);
    }
}

mod state_from_init_args {
    use super::*;

    #[test]
    fn should_succeed_with_valid_args() {
        let state = State::try_from(valid_init_args()).unwrap();

        assert_eq!(
            state,
            State {
                minter_public_key: None,
                master_key_name: Ed25519KeyName::MainnetProdKey1,
                ledger_canister_id: ledger_canister_id(),
                sol_rpc_canister_id: sol_rpc_canister_id(),
                deposit_fee: DEPOSIT_FEE,
                minimum_withdrawal_amount: MINIMUM_WITHDRAWAL_AMOUNT,
                minimum_deposit_amount: MINIMUM_DEPOSIT_AMOUNT,
                pending_update_balance_requests: BTreeSet::new(),
                accepted_deposits: BTreeMap::new(),
                quarantined_deposit: BTreeSet::new(),
                minted_deposits: BTreeMap::new(),
            }
        );
    }

    #[test]
    fn should_fail_when_any_canister_id_is_anonymous() {
        assert_matches!(
            State::try_from(InitArgs {
                sol_rpc_canister_id: Principal::anonymous(),
                ..valid_init_args()
            }),
            Err(InvalidStateError::InvalidCanisterId(_))
        );

        assert_matches!(
            State::try_from(InitArgs {
                ledger_canister_id: Principal::anonymous(),
                ..valid_init_args()
            }),
            Err(InvalidStateError::InvalidCanisterId(_))
        );

        assert_matches!(
            State::try_from(InitArgs {
                sol_rpc_canister_id: Principal::anonymous(),
                ledger_canister_id: Principal::anonymous(),
                ..valid_init_args()
            }),
            Err(InvalidStateError::InvalidCanisterId(_))
        );
    }

    #[test]
    fn should_fail_when_canister_ids_are_identical() {
        let same_id = sol_rpc_canister_id();
        let args = InitArgs {
            sol_rpc_canister_id: same_id,
            ledger_canister_id: same_id,
            ..valid_init_args()
        };

        assert_matches!(
            State::try_from(args),
            Err(InvalidStateError::InvalidCanisterId(_))
        );
    }

    #[test]
    fn should_fail_when_minimum_deposit_amount_too_low() {
        let insufficient_minimum_deposit_amount = DEPOSIT_FEE / 2;
        let args = InitArgs {
            minimum_deposit_amount: insufficient_minimum_deposit_amount,
            ..valid_init_args()
        };

        assert_matches!(
            State::try_from(args),
            Err(InvalidStateError::InvalidMinimumDepositAmount {
                minimum_deposit_amount,
                deposit_fee
            }) if minimum_deposit_amount == insufficient_minimum_deposit_amount && deposit_fee == DEPOSIT_FEE
        );
    }
}

mod state_upgrade {
    use super::*;

    fn initial_state() -> State {
        State::try_from(valid_init_args()).unwrap()
    }

    #[test]
    fn should_update_sol_rpc_canister_id() {
        let mut state = initial_state();
        let new_canister_id = Principal::from_slice(&[3_u8; 20]);

        state
            .upgrade(UpgradeArgs {
                sol_rpc_canister_id: Some(new_canister_id),
                ..Default::default()
            })
            .unwrap();

        assert_eq!(state.sol_rpc_canister_id(), new_canister_id);
    }

    #[test]
    fn should_update_deposit_fee() {
        let mut state = initial_state();
        let new_deposit_fee = DEPOSIT_FEE / 2;

        state
            .upgrade(UpgradeArgs {
                deposit_fee: Some(new_deposit_fee),
                ..Default::default()
            })
            .unwrap();

        assert_eq!(state.deposit_fee(), new_deposit_fee);
    }

    #[test]
    fn should_update_minimum_deposit_amount() {
        let mut state = initial_state();
        let new_minimum_deposit_amount = MINIMUM_DEPOSIT_AMOUNT * 2;

        state
            .upgrade(UpgradeArgs {
                minimum_deposit_amount: Some(new_minimum_deposit_amount),
                ..Default::default()
            })
            .unwrap();

        assert_eq!(state.minimum_deposit_amount(), new_minimum_deposit_amount);
    }

    #[test]
    fn should_update_minimum_withdrawal_amount() {
        let mut state = initial_state();
        let new_minimum_withdrawal_amount = MINIMUM_WITHDRAWAL_AMOUNT * 2;

        state
            .upgrade(UpgradeArgs {
                minimum_withdrawal_amount: Some(new_minimum_withdrawal_amount),
                ..Default::default()
            })
            .unwrap();

        assert_eq!(
            state.minimum_withdrawal_amount(),
            new_minimum_withdrawal_amount
        );
    }

    #[test]
    fn should_fail_when_new_sol_rpc_canister_id_is_anonymous() {
        let mut state = initial_state();

        assert_matches!(
            state.upgrade(UpgradeArgs {
                sol_rpc_canister_id: Some(Principal::anonymous()),
                ..Default::default()
            }),
            Err(InvalidStateError::InvalidCanisterId(_))
        );
    }

    #[test]
    fn should_fail_when_new_deposit_fee_exceeds_minimum_deposit_amount() {
        let mut state = initial_state();
        let new_deposit_fee = MINIMUM_DEPOSIT_AMOUNT + 1;

        assert_matches!(
            state.upgrade(UpgradeArgs {
                deposit_fee: Some(new_deposit_fee),
                ..Default::default()
            }),
            Err(InvalidStateError::InvalidMinimumDepositAmount {
                minimum_deposit_amount,
                deposit_fee
            }) if minimum_deposit_amount == MINIMUM_DEPOSIT_AMOUNT && deposit_fee == new_deposit_fee
        );
    }

    #[test]
    fn should_fail_when_new_minimum_deposit_amount_below_deposit_fee() {
        let mut state = initial_state();
        let new_minimum_deposit_amount = DEPOSIT_FEE - 1;

        assert_matches!(
            state.upgrade(UpgradeArgs {
                minimum_deposit_amount: Some(new_minimum_deposit_amount),
                ..Default::default()
            }),
            Err(InvalidStateError::InvalidMinimumDepositAmount {
                minimum_deposit_amount,
                deposit_fee
            }) if minimum_deposit_amount == new_minimum_deposit_amount && deposit_fee == DEPOSIT_FEE
        );
    }

    #[test]
    fn should_succeed_when_minimum_deposit_amount_equals_deposit_fee() {
        let mut state = initial_state();

        state
            .upgrade(UpgradeArgs {
                minimum_deposit_amount: Some(DEPOSIT_FEE),
                ..Default::default()
            })
            .unwrap();

        assert_eq!(state.minimum_deposit_amount(), DEPOSIT_FEE);
    }

    // This test ensures the canister state is rolled back after a failed upgrade
    #[test]
    #[should_panic = "InvalidMinimumDepositAmount"]
    fn should_panic_when_upgrade_fails() {
        let mut state = initial_state();
        let new_minimum_deposit_amount = DEPOSIT_FEE - 1;

        process_event(
            &mut state,
            EventType::Upgrade(UpgradeArgs {
                minimum_deposit_amount: Some(new_minimum_deposit_amount),
                ..Default::default()
            }),
            &TestCanisterRuntime::new(),
        );
    }
}
