use super::*;
use crate::{
    state::{
        audit::replay_events,
        event::{Event, EventType},
    },
    test_fixtures::valid_init_args,
};
use cksol_types_internal::UpgradeArgs;
use ic_stable_structures::Storable;
use std::borrow::Cow;

fn state() -> State {
    State::try_from(valid_init_args()).unwrap()
}

fn registration() -> SplTokenRegistration {
    SplTokenRegistration {
        mint: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".into(),
        token_program: CLASSIC_TOKEN_PROGRAM.to_string(),
        decimals: 6,
        ledger_id: Principal::from_slice(&[42, 1]),
        index_id: Some(Principal::from_slice(&[43, 1])),
        symbol: "ckTestUSDC".into(),
        minimum_deposit_amount: 100,
        deposit_fee: 10,
        deposits_paused: false,
    }
}

fn add(token: SplTokenRegistration) -> UpgradeArgs {
    UpgradeArgs {
        add_spl_tokens: Some(vec![token]),
        ..Default::default()
    }
}

fn update(token: &SplTokenRegistration) -> SplTokenUpdate {
    SplTokenUpdate {
        mint: token.mint.clone(),
        minimum_deposit_amount: None,
        deposit_fee: None,
        deposits_paused: None,
    }
}

#[test]
fn should_register_and_discover_tokens_without_changing_sol_configuration() {
    let mut state = state();
    let before = state.clone();
    assert!(state.supported_spl_tokens().is_empty());
    let token = registration();
    state.upgrade(add(token.clone())).unwrap();
    let info = state.supported_spl_tokens();
    assert_eq!(info.len(), 1);
    assert_eq!(
        info[0],
        SplTokenInfo {
            mint: parse_address(&token.mint).unwrap().into(),
            token_program: CLASSIC_TOKEN_PROGRAM.into(),
            decimals: token.decimals,
            ledger_id: token.ledger_id,
            index_id: token.index_id,
            symbol: token.symbol,
            minimum_deposit_amount: token.minimum_deposit_amount,
            deposit_fee: token.deposit_fee,
            deposits_paused: token.deposits_paused,
        }
    );
    state.supported_spl_tokens.clear();
    assert_eq!(state, before);
}

#[test]
fn should_preserve_omitted_settings_and_apply_explicit_zero_and_false() {
    let mut state = state();
    let mut token = registration();
    token.deposits_paused = true;
    state.upgrade(add(token.clone())).unwrap();
    let registered = state.clone();
    state.upgrade(UpgradeArgs::default()).unwrap();
    assert_eq!(state, registered);
    state
        .upgrade(UpgradeArgs {
            update_spl_tokens: Some(vec![update(&token)]),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(state, registered);
    state
        .upgrade(UpgradeArgs {
            update_spl_tokens: Some(vec![SplTokenUpdate {
                minimum_deposit_amount: Some(500),
                deposit_fee: Some(0),
                deposits_paused: Some(false),
                ..update(&token)
            }]),
            ..Default::default()
        })
        .unwrap();
    let info = state.supported_spl_tokens();
    assert_eq!(info[0].minimum_deposit_amount, 500);
    assert_eq!(info[0].deposit_fee, 0);
    assert!(!info[0].deposits_paused);
    assert_eq!(info[0].ledger_id, token.ledger_id);
}

#[test]
fn should_reject_invalid_registrations() {
    let mut invalid_tokens = Vec::new();
    let mut token = registration();
    token.mint = "not-an-address".into();
    invalid_tokens.push(token);
    let mut token = registration();
    token.token_program = "11111111111111111111111111111111".into();
    invalid_tokens.push(token);
    let mut token = registration();
    token.token_program = "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb".into();
    invalid_tokens.push(token);
    let mut token = registration();
    token.symbol = " ".into();
    invalid_tokens.push(token);
    let mut token = registration();
    token.symbol = "é".into();
    invalid_tokens.push(token);
    let mut token = registration();
    token.symbol = "x".repeat(21);
    invalid_tokens.push(token);
    let mut token = registration();
    token.minimum_deposit_amount = 0;
    invalid_tokens.push(token);
    let mut token = registration();
    token.minimum_deposit_amount = u64::MAX;
    invalid_tokens.push(token);
    let mut token = registration();
    token.ledger_id = Principal::anonymous();
    invalid_tokens.push(token);
    let mut token = registration();
    token.index_id = Some(Principal::management_canister());
    invalid_tokens.push(token);
    let mut token = registration();
    token.index_id = Some(token.ledger_id);
    invalid_tokens.push(token);
    let mut token = registration();
    token.ledger_id = state().ledger_canister_id();
    invalid_tokens.push(token);

    for invalid_token in invalid_tokens {
        let mut state = state();
        let mut args = add(invalid_token.clone());
        args.withdrawal_fee = Some(1);
        assert!(
            matches!(
                state.upgrade(args),
                Err(InvalidStateError::InvalidSplToken(_))
            ),
            "{invalid_token:?}"
        );
    }
}

#[test]
fn should_reject_duplicate_and_unknown_mints_without_partial_registration() {
    let token = registration();
    let cases = [
        UpgradeArgs {
            add_spl_tokens: Some(vec![token.clone(), token.clone()]),
            ..Default::default()
        },
        UpgradeArgs {
            update_spl_tokens: Some(vec![update(&token)]),
            ..Default::default()
        },
        UpgradeArgs {
            add_spl_tokens: Some(vec![token.clone()]),
            update_spl_tokens: Some(vec![update(&token)]),
            ..Default::default()
        },
    ];
    for args in cases {
        let mut state = state();
        let before = state.clone();
        assert!(matches!(
            state.upgrade(args),
            Err(InvalidStateError::InvalidSplToken(_))
        ));
        assert_eq!(state, before);
    }
    let mut state = state();
    state.upgrade(add(token.clone())).unwrap();
    let before = state.clone();
    assert!(state.upgrade(add(token)).is_err());
    assert_eq!(state, before);
}

#[test]
fn should_reject_conflicting_or_unknown_updates_without_applying_them() {
    let token = registration();
    let mut state = state();
    state.upgrade(add(token.clone())).unwrap();
    let before = state.clone();
    let cases = [
        vec![
            SplTokenUpdate {
                deposit_fee: Some(0),
                ..update(&token)
            },
            update(&token),
        ],
        vec![
            SplTokenUpdate {
                deposit_fee: Some(0),
                ..update(&token)
            },
            SplTokenUpdate {
                mint: Address::from([2; 32]).to_string(),
                ..update(&token)
            },
        ],
    ];
    for updates in cases {
        assert!(
            state
                .upgrade(UpgradeArgs {
                    update_spl_tokens: Some(updates),
                    ..Default::default()
                })
                .is_err()
        );
        assert_eq!(state, before);
    }
}

#[test]
fn should_replay_registration_and_updates_from_serialized_upgrade_events() {
    let token = registration();
    let events = [
        Event {
            timestamp: 0,
            payload: EventType::Init(valid_init_args()),
        },
        Event {
            timestamp: 1,
            payload: EventType::Upgrade(add(token.clone())),
        },
        Event {
            timestamp: 2,
            payload: EventType::Upgrade(UpgradeArgs {
                update_spl_tokens: Some(vec![SplTokenUpdate {
                    deposit_fee: Some(0),
                    deposits_paused: Some(true),
                    ..update(&token)
                }]),
                ..Default::default()
            }),
        },
    ];
    let decoded = events
        .iter()
        .map(|event| Event::from_bytes(Cow::Owned(event.to_bytes().into_owned())));
    let replayed = replay_events(decoded);
    let mut expected = state();
    expected.upgrade(add(token.clone())).unwrap();
    expected
        .upgrade(UpgradeArgs {
            update_spl_tokens: Some(vec![SplTokenUpdate {
                deposit_fee: Some(0),
                deposits_paused: Some(true),
                ..update(&token)
            }]),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(replayed, expected);
    assert_eq!(replayed.supported_spl_tokens()[0].deposit_fee, 0);
    assert!(replayed.supported_spl_tokens()[0].deposits_paused);
}

#[test]
fn should_decode_pre_spl_upgrade_arguments_with_an_empty_registry() {
    #[derive(minicbor::Encode)]
    struct LegacyUpgradeArgs {
        #[n(7)]
        automatic_deposit_fee: Option<u64>,
    }
    let bytes = minicbor::to_vec(LegacyUpgradeArgs {
        automatic_deposit_fee: Some(100),
    })
    .unwrap();
    let args: UpgradeArgs = minicbor::decode(&bytes).unwrap();
    assert!(args.add_spl_tokens.is_none());
    assert!(args.update_spl_tokens.is_none());
    let mut state = state();
    state.upgrade(args).unwrap();
    assert!(state.supported_spl_tokens().is_empty());
    assert_eq!(state.automatic_deposit_fee(), Some(100));
}

#[test]
fn should_list_distinct_mints_in_order_and_allow_an_omitted_index() {
    let first = registration();
    let mut second = registration();
    second.mint = Address::from([2; 32]).to_string();
    second.ledger_id = Principal::from_slice(&[44, 1]);
    second.index_id = None;
    second.deposit_fee = 0;
    let mut state = state();
    state
        .upgrade(UpgradeArgs {
            add_spl_tokens: Some(vec![first.clone(), second.clone()]),
            ..Default::default()
        })
        .unwrap();
    let info = state.supported_spl_tokens();
    let mut expected = [
        parse_address(&first.mint).unwrap(),
        parse_address(&second.mint).unwrap(),
    ];
    expected.sort();
    assert_eq!(
        info.iter()
            .map(|token| Address::from(token.mint.clone()))
            .collect::<Vec<_>>(),
        expected
    );
    assert_eq!(
        info.iter()
            .find(|token| token.ledger_id == second.ledger_id)
            .unwrap()
            .index_id,
        None
    );
}

#[test]
fn should_validate_registry_against_the_proposed_rpc_id() {
    let token = registration();
    for already_registered in [false, true] {
        let mut state = state();
        if already_registered {
            state.upgrade(add(token.clone())).unwrap();
        }
        let args = UpgradeArgs {
            sol_rpc_canister_id: Some(token.ledger_id),
            add_spl_tokens: (!already_registered).then(|| vec![token.clone()]),
            ..Default::default()
        };
        assert!(matches!(
            state.upgrade(args),
            Err(InvalidStateError::InvalidSplToken(_))
        ));
    }
}

#[test]
fn should_validate_stored_token_settings_without_upgrade_arguments() {
    let mut valid = state();
    valid.upgrade(add(registration())).unwrap();
    valid.validate().unwrap();

    let invalid_settings: Vec<fn(&mut SupportedSplToken)> = vec![
        |token| token.token_program = Address::default(),
        |token| token.ledger_id = Principal::anonymous(),
        |token| token.index_id = Some(Principal::management_canister()),
        |token| token.symbol.clear(),
        |token| token.symbol = "é".into(),
        |token| token.symbol = "x".repeat(MAX_SYMBOL_BYTES + 1),
        |token| token.minimum_deposit_amount = 0,
        |token| token.deposit_fee = u64::MAX,
        |token| token.index_id = Some(token.ledger_id),
    ];
    for (case, invalidate) in invalid_settings.into_iter().enumerate() {
        let mut state = valid.clone();
        invalidate(state.supported_spl_tokens.values_mut().next().unwrap());
        assert!(
            matches!(state.validate(), Err(InvalidStateError::InvalidSplToken(_))),
            "invalid stored token settings accepted in case {case}"
        );
    }
}

#[test]
fn should_validate_stored_registry_canister_id_collisions() {
    let mut state = state();
    let first = registration();
    let mut second = registration();
    second.mint = Address::new_from_array([7; 32]).to_string();
    second.ledger_id = Principal::from_slice(&[44, 1]);
    second.index_id = None;
    state
        .upgrade(UpgradeArgs {
            add_spl_tokens: Some(vec![first.clone(), second.clone()]),
            ..Default::default()
        })
        .unwrap();

    for conflicting_id in [
        first.ledger_id,
        first.index_id.unwrap(),
        state.ledger_canister_id(),
        state.sol_rpc_canister_id(),
    ] {
        state
            .supported_spl_tokens
            .get_mut(&parse_address(&second.mint).unwrap())
            .unwrap()
            .ledger_id = conflicting_id;
        assert!(matches!(
            state.validate(),
            Err(InvalidStateError::InvalidSplToken(_))
        ));
    }
}

#[test]
fn should_reject_invalid_deposit_settings_after_applying_upgrade_arguments() {
    let token = registration();
    for invalid_update in [
        SplTokenUpdate {
            minimum_deposit_amount: Some(0),
            ..update(&token)
        },
        SplTokenUpdate {
            deposit_fee: Some(u64::MAX),
            ..update(&token)
        },
    ] {
        let mut state = state();
        state.upgrade(add(token.clone())).unwrap();
        // The lifecycle traps on this error; IC rollback is outside this unit test.
        assert!(matches!(
            state.upgrade(UpgradeArgs {
                update_spl_tokens: Some(vec![invalid_update]),
                ..Default::default()
            }),
            Err(InvalidStateError::InvalidSplToken(_))
        ));
    }
}

#[test]
fn should_reject_registrations_with_shared_canister_ids() {
    let first = registration();
    for conflicting_id in [first.ledger_id, first.index_id.unwrap()] {
        let mut second = registration();
        second.mint = Address::from([2; 32]).to_string();
        second.ledger_id = conflicting_id;
        second.index_id = None;
        let mut state = state();
        assert!(matches!(
            state.upgrade(UpgradeArgs {
                add_spl_tokens: Some(vec![first.clone(), second]),
                ..Default::default()
            }),
            Err(InvalidStateError::InvalidSplToken(_))
        ));
    }
}
