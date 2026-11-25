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
        withdrawal_fee_token: None,
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
        withdrawal_fee_token: None,
    }
}

#[test]
fn should_register_and_discover_tokens_without_changing_sol_configuration() {
    let mut state = state();
    let before = state.clone();
    assert!(state.supported_spl_tokens().is_empty());
    assert!(state.collected_spl_deposit_fees().is_empty());
    let token = registration();
    state.upgrade(add(token.clone())).unwrap();
    assert_eq!(
        state.collected_spl_deposit_fees(),
        [CollectedSplDepositFees {
            mint: parse_address(&token.mint).unwrap().into(),
            amount: 0,
        }]
    );
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
            withdrawal_fee_token: 0,
            minimum_withdrawal_amount: 1,
            withdrawal_fee_cksol: 1_000_000,
            refundable_rent: 2_039_280,
            total_withdrawal_fee_cksol: 3_039_280,
        }
    );
    assert_eq!(
        state.all_collected_spl_withdrawal_fees(),
        [cksol_types::CollectedSplWithdrawalFees {
            mint: parse_address(&token.mint).unwrap().into(),
            amount: 0,
        }]
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
fn should_register_and_update_the_token_withdrawal_fee() {
    let mut state = state();
    let token = registration();
    assert_eq!(token.withdrawal_fee_token, None);
    let other = SplTokenRegistration {
        mint: "So11111111111111111111111111111111111111112".into(),
        ledger_id: Principal::from_slice(&[44, 1]),
        index_id: None,
        withdrawal_fee_token: Some(25),
        ..registration()
    };
    state
        .upgrade(UpgradeArgs {
            add_spl_tokens: Some(vec![token.clone(), other.clone()]),
            ..Default::default()
        })
        .unwrap();
    let fee = |state: &State, token: &SplTokenRegistration| {
        state
            .supported_spl_token(&parse_address(&token.mint).unwrap())
            .unwrap()
            .withdrawal_fee_token
    };
    // An omitted fee is no fee.
    assert_eq!(fee(&state, &token), 0);
    assert_eq!(fee(&state, &other), 25);

    for (new_fee, expected) in [
        (Some(7), 7),
        (None, 7),
        (Some(0), 0),
        (Some(u64::MAX), u64::MAX),
    ] {
        let deposit_settings = state.supported_spl_tokens();
        state
            .upgrade(UpgradeArgs {
                update_spl_tokens: Some(vec![SplTokenUpdate {
                    withdrawal_fee_token: new_fee,
                    ..update(&token)
                }]),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(fee(&state, &token), expected);
        // The other token and the deposit settings keep their values.
        assert_eq!(fee(&state, &other), 25);
        for (before, after) in deposit_settings.iter().zip(state.supported_spl_tokens()) {
            assert_eq!(
                SplTokenInfo {
                    withdrawal_fee_token: after.withdrawal_fee_token,
                    minimum_withdrawal_amount: after.minimum_withdrawal_amount,
                    ..before.clone()
                },
                after
            );
        }
    }
    // An update of the deposit settings keeps the withdrawal fee.
    state
        .upgrade(UpgradeArgs {
            update_spl_tokens: Some(vec![SplTokenUpdate {
                minimum_deposit_amount: Some(500),
                deposit_fee: Some(1),
                deposits_paused: Some(true),
                ..update(&token)
            }]),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(fee(&state, &token), u64::MAX);

    // The discovery lists the tokens in mint-address order.
    let info = state.supported_spl_tokens();
    assert_eq!(
        info.iter()
            .map(|token| token.withdrawal_fee_token)
            .collect::<Vec<_>>(),
        [25, u64::MAX]
    );
}

#[test]
fn should_default_and_update_the_spl_withdrawal_fee_without_changing_the_sol_fee() {
    use crate::constants::FEE_PER_SIGNATURE;
    let mut state = state();
    let sol_fee = state.withdrawal_fee();
    assert_eq!(state.spl_withdrawal_fee(), 1_000_000);
    assert_eq!(state.spl_withdrawal_rent_reserve(), 2_039_280);

    // An omitted fee keeps the value.
    state.upgrade(UpgradeArgs::default()).unwrap();
    assert_eq!(state.spl_withdrawal_fee(), 1_000_000);
    for fee in [FEE_PER_SIGNATURE, 2_000_000, u64::MAX] {
        state
            .upgrade(UpgradeArgs {
                spl_withdrawal_fee: Some(fee),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(state.spl_withdrawal_fee(), fee);
        assert_eq!(state.withdrawal_fee(), sol_fee);
    }

    // The fee must cover the transaction fee that the minter pays. A rejected upgrade
    // traps in the canister, so each attempt starts from a copy of the valid state.
    for fee in [0, FEE_PER_SIGNATURE - 1] {
        assert_eq!(
            state.clone().upgrade(UpgradeArgs {
                spl_withdrawal_fee: Some(fee),
                ..Default::default()
            }),
            Err(InvalidStateError::SplWithdrawalFeeTooLow {
                spl_withdrawal_fee: fee,
                transaction_fee: FEE_PER_SIGNATURE,
            })
        );
    }
}

#[test]
fn should_report_the_cksol_withdrawal_fees_with_each_token() {
    let mut state = state();
    state.upgrade(add(registration())).unwrap();
    let fees = |state: &State| {
        let info = &state.supported_spl_tokens()[0];
        (
            info.withdrawal_fee_cksol,
            info.refundable_rent,
            info.total_withdrawal_fee_cksol,
        )
    };
    assert_eq!(fees(&state), (1_000_000, 2_039_280, 3_039_280));

    // The token info follows the global fee setting.
    for (fee, total) in [(2_000_000, 4_039_280), (u64::MAX, u64::MAX)] {
        state
            .upgrade(UpgradeArgs {
                spl_withdrawal_fee: Some(fee),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(fees(&state), (fee, 2_039_280, total));
    }
}

#[test]
fn should_report_the_minimum_withdrawal_amount_with_each_token() {
    for (withdrawal_fee_token, minimum) in [(None, 1), (Some(25), 26), (Some(u64::MAX), u64::MAX)] {
        let mut state = state();
        state
            .upgrade(add(SplTokenRegistration {
                withdrawal_fee_token,
                ..registration()
            }))
            .unwrap();
        assert_eq!(
            state.supported_spl_tokens()[0].minimum_withdrawal_amount,
            minimum
        );
    }
}

#[test]
fn should_replay_withdrawal_settings_from_serialized_upgrade_events() {
    let token = SplTokenRegistration {
        withdrawal_fee_token: Some(3),
        ..registration()
    };
    let upgrades = [
        add(token.clone()),
        UpgradeArgs {
            update_spl_tokens: Some(vec![SplTokenUpdate {
                withdrawal_fee_token: Some(9),
                ..update(&token)
            }]),
            spl_withdrawal_fee: Some(1_500_000),
            ..Default::default()
        },
    ];
    let events: Vec<_> = std::iter::once(EventType::Init(valid_init_args()))
        .chain(upgrades.iter().cloned().map(EventType::Upgrade))
        .enumerate()
        .map(|(timestamp, payload)| Event {
            timestamp: timestamp as u64,
            payload,
        })
        .collect();
    let decoded = events
        .iter()
        .map(|event| Event::from_bytes(Cow::Owned(event.to_bytes().into_owned())));
    let replayed = replay_events(decoded);
    let mut expected = state();
    for upgrade in upgrades {
        expected.upgrade(upgrade).unwrap();
    }
    assert_eq!(replayed, expected);
    assert_eq!(replayed.supported_spl_tokens()[0].withdrawal_fee_token, 9);
    assert_eq!(replayed.spl_withdrawal_fee(), 1_500_000);
}

#[test]
fn should_decode_registrations_and_updates_recorded_without_withdrawal_settings() {
    #[derive(minicbor::Encode)]
    struct LegacyRegistration {
        #[n(0)]
        mint: String,
        #[n(1)]
        token_program: String,
        #[n(2)]
        decimals: u8,
        #[n(3)]
        #[cbor(with = "icrc_cbor::principal")]
        ledger_id: Principal,
        #[n(5)]
        symbol: String,
        #[n(6)]
        minimum_deposit_amount: u64,
        #[n(7)]
        deposit_fee: u64,
        #[n(8)]
        deposits_paused: bool,
    }
    #[derive(minicbor::Encode)]
    struct LegacyUpdate {
        #[n(0)]
        mint: String,
        #[n(2)]
        deposit_fee: Option<u64>,
    }
    #[derive(minicbor::Encode)]
    struct LegacyUpgradeArgs {
        #[n(8)]
        add_spl_tokens: Option<Vec<LegacyRegistration>>,
        #[n(9)]
        update_spl_tokens: Option<Vec<LegacyUpdate>>,
        #[n(11)]
        spl_deposit_consolidation_fee: Option<u64>,
    }
    let token = registration();
    let decode = |legacy: LegacyUpgradeArgs| -> UpgradeArgs {
        minicbor::decode(&minicbor::to_vec(legacy).unwrap()).unwrap()
    };
    let registered = decode(LegacyUpgradeArgs {
        add_spl_tokens: Some(vec![LegacyRegistration {
            mint: token.mint.clone(),
            token_program: token.token_program.clone(),
            decimals: token.decimals,
            ledger_id: token.ledger_id,
            symbol: token.symbol.clone(),
            minimum_deposit_amount: token.minimum_deposit_amount,
            deposit_fee: token.deposit_fee,
            deposits_paused: token.deposits_paused,
        }]),
        update_spl_tokens: None,
        spl_deposit_consolidation_fee: Some(1),
    });
    assert_eq!(registered.spl_withdrawal_fee, None);
    assert_eq!(
        registered.add_spl_tokens,
        Some(vec![SplTokenRegistration {
            index_id: None,
            ..token.clone()
        }])
    );
    let updated = decode(LegacyUpgradeArgs {
        add_spl_tokens: None,
        update_spl_tokens: Some(vec![LegacyUpdate {
            mint: token.mint.clone(),
            deposit_fee: Some(4),
        }]),
        spl_deposit_consolidation_fee: None,
    });
    assert_eq!(
        updated.update_spl_tokens,
        Some(vec![SplTokenUpdate {
            deposit_fee: Some(4),
            ..update(&token)
        }])
    );

    let mut state = state();
    state.upgrade(registered).unwrap();
    state.upgrade(updated).unwrap();
    assert_eq!(state.supported_spl_tokens()[0].withdrawal_fee_token, 0);
    assert_eq!(state.supported_spl_tokens()[0].deposit_fee, 4);
    assert_eq!(state.spl_withdrawal_fee(), 1_000_000);
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
