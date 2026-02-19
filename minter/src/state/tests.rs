use super::{event::*, *};
use crate::test_fixtures::{
    DEPOSIT_FEE, arb::arb_event, ledger_canister_id, sol_rpc_canister_id, valid_init_args,
};
use assert_matches::assert_matches;
use cksol_types_internal::{Ed25519KeyName, InitArgs};
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
                pending_update_balance_requests: BTreeSet::new(),
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
}
