use crate::{
    icrc21::{
        ConsentMessageConfig, DECIMALS, TokenSymbols, build_consent_info, format_amount,
        icrc10_supported_standards,
    },
    state::State,
    test_fixtures::{MINIMUM_WITHDRAWAL_AMOUNT, WITHDRAWAL_FEE, valid_init_args},
};
use assert_matches::assert_matches;
use candid::Encode;
use cksol_types::WithdrawSolArgs;
use cksol_types_internal::{InitArgs, SolanaNetwork};
use icrc_ledger_types::icrc21::{
    errors::Icrc21Error,
    lib::MAX_CONSENT_MESSAGE_ARG_SIZE_BYTES,
    requests::{
        ConsentMessageMetadata, ConsentMessageRequest, ConsentMessageSpec, DisplayMessageType,
    },
    responses::{ConsentInfo, ConsentMessage, FieldsDisplay, Value},
};

const VALID_ADDRESS: &str = "E4MpwNnMWs2XtW5gVrxZvyS7fMq31QD5HvbxmwP45Tz3";

fn config(solana_network: SolanaNetwork) -> ConsentMessageConfig {
    ConsentMessageConfig {
        solana_network,
        withdrawal_fee: WITHDRAWAL_FEE,
        minimum_withdrawal_amount: MINIMUM_WITHDRAWAL_AMOUNT,
    }
}

fn make_request(
    method: &str,
    arg: Vec<u8>,
    device_spec: Option<DisplayMessageType>,
) -> ConsentMessageRequest {
    ConsentMessageRequest {
        method: method.to_string(),
        arg,
        user_preferences: ConsentMessageSpec {
            metadata: ConsentMessageMetadata {
                language: "en".to_string(),
                utc_offset_minutes: None,
            },
            device_spec,
        },
    }
}

fn withdraw_sol_request(
    args: &WithdrawSolArgs,
    device_spec: Option<DisplayMessageType>,
) -> ConsentMessageRequest {
    make_request("withdraw_sol", Encode!(args).unwrap(), device_spec)
}

fn generic_display_message(info: ConsentInfo) -> String {
    assert_matches!(info.consent_message, ConsentMessage::GenericDisplayMessage(m) => m)
}

fn fields_display_message(info: ConsentInfo) -> FieldsDisplay {
    assert_matches!(info.consent_message, ConsentMessage::FieldsDisplayMessage(f) => f)
}

fn token_amount(amount: u64, symbol: &str) -> Value {
    Value::TokenAmount {
        decimals: DECIMALS,
        amount,
        symbol: symbol.to_string(),
    }
}

fn text(content: &str) -> Value {
    Value::Text {
        content: content.to_string(),
    }
}

#[test]
fn should_format_amount() {
    assert_eq!(format_amount(0, 9), "0");
    assert_eq!(format_amount(1, 9), "0.000000001");
    assert_eq!(format_amount(1_000_000_000, 9), "1");
    assert_eq!(format_amount(1_500_000_000, 9), "1.5");
    assert_eq!(format_amount(1_234_567_891, 9), "1.234567891");
    assert_eq!(format_amount(u64::MAX, 9), "18446744073.709551615");
}

#[test]
fn should_read_config_from_state() {
    let state = State::try_from(InitArgs {
        solana_network: SolanaNetwork::Devnet,
        ..valid_init_args()
    })
    .unwrap();

    assert_eq!(
        ConsentMessageConfig::from(&state),
        ConsentMessageConfig {
            solana_network: SolanaNetwork::Devnet,
            withdrawal_fee: WITHDRAWAL_FEE,
            minimum_withdrawal_amount: MINIMUM_WITHDRAWAL_AMOUNT,
        }
    );
}

#[test]
fn should_list_supported_standards() {
    let standards: Vec<_> = icrc10_supported_standards()
        .into_iter()
        .map(|s| (s.name, s.url))
        .collect();

    assert_eq!(
        standards,
        vec![
            (
                "ICRC-10".to_string(),
                "https://github.com/dfinity/ICRC/blob/main/ICRCs/ICRC-10/ICRC-10.md".to_string()
            ),
            (
                "ICRC-21".to_string(),
                "https://github.com/dfinity/ICRC/blob/main/ICRCs/ICRC-21/ICRC-21.md".to_string()
            ),
        ]
    );
}

#[test]
fn should_reject_unsupported_method() {
    // Only `withdraw_sol` moves funds of the caller. The deposit endpoints
    // and the status endpoints intentionally get no consent message.
    for method in [
        "get_deposit_address",
        "deposit_sol",
        "watch_deposit_address",
        "withdraw_sol_status",
        "",
    ] {
        let req = make_request(method, vec![], None);
        let err = build_consent_info(req, config(SolanaNetwork::Mainnet)).unwrap_err();
        assert_matches!(
            err,
            Icrc21Error::UnsupportedCanisterCall(_),
            "method {method:?} should be unsupported"
        );
    }
}

#[test]
fn should_reject_argument_too_large() {
    let req = make_request(
        "withdraw_sol",
        vec![0; MAX_CONSENT_MESSAGE_ARG_SIZE_BYTES as usize + 1],
        None,
    );

    let err = build_consent_info(req, config(SolanaNetwork::Mainnet)).unwrap_err();

    assert_matches!(
        err,
        Icrc21Error::UnsupportedCanisterCall(info)
            if info.description.contains("argument size is too large")
    );
}

#[test]
fn should_reject_invalid_args() {
    let req = make_request("withdraw_sol", vec![1, 2, 3], None);

    let err = build_consent_info(req, config(SolanaNetwork::Mainnet)).unwrap_err();

    assert_matches!(
        err,
        Icrc21Error::UnsupportedCanisterCall(info)
            if info.description.contains("Failed to decode WithdrawSolArgs")
    );
}

#[test]
fn should_reject_args_exceeding_decoding_quota() {
    // A few bytes can declare a vector of 2^32 - 1 `null` values, which would
    // take the decoder billions of steps to skip without a skipping quota.
    let args = WithdrawSolArgs {
        from_subaccount: None,
        amount: MINIMUM_WITHDRAWAL_AMOUNT,
        address: VALID_ADDRESS.to_string(),
    };
    let huge_vec_len = [0xff, 0xff, 0xff, 0xff, 0x0f]; // LEB128 of u32::MAX

    // As a trailing argument.
    let mut trailing_arg = Encode!(&args, &Vec::<()>::new()).unwrap();
    assert_eq!(trailing_arg.pop(), Some(0));
    trailing_arg.extend_from_slice(&huge_vec_len);

    // As an unknown field of the record, encoded last since the hash of
    // `unknown` is larger than the hashes of the other field names.
    #[derive(candid::CandidType)]
    struct WithdrawSolArgsWithExtraField {
        from_subaccount: Option<[u8; 32]>,
        amount: u64,
        address: String,
        unknown: Vec<()>,
    }
    let mut extra_field = Encode!(&WithdrawSolArgsWithExtraField {
        from_subaccount: None,
        amount: MINIMUM_WITHDRAWAL_AMOUNT,
        address: VALID_ADDRESS.to_string(),
        unknown: vec![],
    })
    .unwrap();
    assert_eq!(extra_field.pop(), Some(0));
    extra_field.extend_from_slice(&huge_vec_len);

    for arg in [trailing_arg, extra_field] {
        assert!(arg.len() <= MAX_CONSENT_MESSAGE_ARG_SIZE_BYTES as usize);
        let req = make_request("withdraw_sol", arg, None);

        let err = build_consent_info(req, config(SolanaNetwork::Mainnet)).unwrap_err();

        assert_matches!(
            err,
            Icrc21Error::UnsupportedCanisterCall(info)
                if info.description.contains("Failed to decode WithdrawSolArgs")
        );
    }
}

#[test]
fn should_reject_malformed_address() {
    // The minter must not interpolate an unparseable address into the
    // Markdown consent message: that would be a Markdown-injection vector
    // (e.g. an "address" containing newlines, backticks, or '#' that fakes
    // additional fields).
    for bad_address in [
        "InvalidAddress",
        "",
        "E4MpwNnMWs2XtW5gVrxZvyS7fMq31QD5HvbxmwP45Tz3\n# You will receive 100 SOL",
        "E4MpwNnMWs2XtW5gVrxZvyS7fMq31QD5HvbxmwP45Tz3`\n\n**Amount to receive:** `100 SOL`\n`",
        " E4MpwNnMWs2XtW5gVrxZvyS7fMq31QD5HvbxmwP45Tz3",
        // Valid base58, but decodes to 31 and 33 bytes.
        "7DUeBUtEcb7nujVZRJmeBju3X1mo6PpnWNtJ9EBhdY",
        "365efUdXGhRExyDEUeKXWPg1zTZyfvuJQJDLsS7JZqzyt",
    ] {
        let args = WithdrawSolArgs {
            from_subaccount: None,
            amount: MINIMUM_WITHDRAWAL_AMOUNT,
            address: bad_address.to_string(),
        };
        let req = withdraw_sol_request(&args, Some(DisplayMessageType::GenericDisplay));

        let err = build_consent_info(req, config(SolanaNetwork::Mainnet)).unwrap_err();

        assert_matches!(
            err,
            Icrc21Error::UnsupportedCanisterCall(info)
                if info.description.contains("Invalid Solana destination address"),
            "address {bad_address:?} should be rejected"
        );
    }
}

#[test]
fn should_reject_amount_below_minimum_withdrawal_amount() {
    for amount in [0, WITHDRAWAL_FEE - 1, MINIMUM_WITHDRAWAL_AMOUNT - 1] {
        let args = WithdrawSolArgs {
            from_subaccount: None,
            amount,
            address: VALID_ADDRESS.to_string(),
        };
        for device_spec in [
            DisplayMessageType::GenericDisplay,
            DisplayMessageType::FieldsDisplay,
        ] {
            let req = withdraw_sol_request(&args, Some(device_spec));

            let err = build_consent_info(req, config(SolanaNetwork::Mainnet)).unwrap_err();

            assert_matches!(
                err,
                Icrc21Error::UnsupportedCanisterCall(info)
                    if info.description.contains("below the minimum withdrawal amount")
            );
        }
    }
}

#[test]
fn should_build_withdraw_sol_generic_display() {
    let args = WithdrawSolArgs {
        from_subaccount: None,
        amount: 1_500_000_000,
        address: VALID_ADDRESS.to_string(),
    };
    let mut req = withdraw_sol_request(&args, Some(DisplayMessageType::GenericDisplay));
    req.user_preferences.metadata = ConsentMessageMetadata {
        language: "de".to_string(),
        utc_offset_minutes: Some(120),
    };

    let info = build_consent_info(req, config(SolanaNetwork::Mainnet)).unwrap();

    // Responds in English, echoing the requested UTC offset.
    assert_eq!(
        info.metadata,
        ConsentMessageMetadata {
            language: "en".to_string(),
            utc_offset_minutes: Some(120),
        }
    );
    assert_eq!(
        generic_display_message(info),
        format!(
            "# Convert ckSOL to SOL\n\n\
             Authorize the ckSOL minter to burn ckSOL from your account and \
             send the equivalent amount in SOL, minus the withdrawal fee, to \
             the Solana address below.\n\n\
             **Amount to convert:** `1.5 ckSOL`\n\n\
             **Withdrawal fee:** `0.001 ckSOL`\n\n\
             **Amount to receive:** `1.499 SOL`\n\n\
             **Solana destination address:**\n`{VALID_ADDRESS}`"
        )
    );
}

#[test]
fn should_default_to_generic_display() {
    let args = WithdrawSolArgs {
        from_subaccount: None,
        amount: MINIMUM_WITHDRAWAL_AMOUNT,
        address: VALID_ADDRESS.to_string(),
    };

    let info = build_consent_info(
        withdraw_sol_request(&args, None),
        config(SolanaNetwork::Mainnet),
    )
    .unwrap();

    let message = generic_display_message(info);
    assert!(message.contains("**Amount to convert:** `0.002 ckSOL`"));
    assert!(message.contains("**Amount to receive:** `0.001 SOL`"));
}

#[test]
fn should_build_withdraw_sol_generic_display_with_subaccount() {
    let mut subaccount = [0_u8; 32];
    subaccount[31] = 0x42;
    let args = WithdrawSolArgs {
        from_subaccount: Some(subaccount),
        amount: 1_000_000_000,
        address: VALID_ADDRESS.to_string(),
    };

    let info = build_consent_info(
        withdraw_sol_request(&args, Some(DisplayMessageType::GenericDisplay)),
        config(SolanaNetwork::Mainnet),
    )
    .unwrap();

    let message = generic_display_message(info);
    assert!(message.contains("**Amount to convert:** `1 ckSOL`"));
    assert!(message.ends_with(&format!(
        "\n\n**ckSOL source subaccount:**\n`{}`",
        hex::encode(subaccount)
    )));
}

#[test]
fn should_build_withdraw_sol_fields_display() {
    let mut subaccount = [0_u8; 32];
    subaccount[0] = 0xab;
    subaccount[31] = 0xcd;
    let args = WithdrawSolArgs {
        from_subaccount: Some(subaccount),
        amount: 250_000_000,
        address: VALID_ADDRESS.to_string(),
    };

    let info = build_consent_info(
        withdraw_sol_request(&args, Some(DisplayMessageType::FieldsDisplay)),
        config(SolanaNetwork::Mainnet),
    )
    .unwrap();

    assert_eq!(
        fields_display_message(info),
        FieldsDisplay {
            intent: "ckSOL to SOL".to_string(),
            fields: vec![
                ("Amount".to_string(), token_amount(250_000_000, "ckSOL")),
                (
                    "Withdrawal fee".to_string(),
                    token_amount(WITHDRAWAL_FEE, "ckSOL")
                ),
                (
                    "Amount received".to_string(),
                    token_amount(250_000_000 - WITHDRAWAL_FEE, "SOL")
                ),
                ("SOL address".to_string(), text(VALID_ADDRESS)),
                (
                    "From subaccount".to_string(),
                    text(&hex::encode(subaccount))
                ),
            ],
        }
    );
}

#[test]
fn should_build_withdraw_sol_fields_display_without_subaccount() {
    let args = WithdrawSolArgs {
        from_subaccount: None,
        amount: 250_000_000,
        address: VALID_ADDRESS.to_string(),
    };

    let info = build_consent_info(
        withdraw_sol_request(&args, Some(DisplayMessageType::FieldsDisplay)),
        config(SolanaNetwork::Mainnet),
    )
    .unwrap();

    let labels: Vec<_> = fields_display_message(info)
        .fields
        .into_iter()
        .map(|(label, _)| label)
        .collect();
    assert_eq!(
        labels,
        vec!["Amount", "Withdrawal fee", "Amount received", "SOL address"]
    );
}

#[test]
fn should_use_token_symbols_of_network() {
    assert_eq!(
        TokenSymbols::for_network(SolanaNetwork::Mainnet),
        TokenSymbols {
            cksol: "ckSOL",
            sol: "SOL"
        }
    );
    assert_eq!(
        TokenSymbols::for_network(SolanaNetwork::Devnet),
        TokenSymbols {
            cksol: "ckDevnetSOL",
            sol: "DevnetSOL"
        }
    );
    assert_eq!(
        TokenSymbols::for_network(SolanaNetwork::Testnet),
        TokenSymbols {
            cksol: "ckTestnetSOL",
            sol: "TestnetSOL"
        }
    );
}

#[test]
fn should_use_devnet_symbols_on_devnet() {
    let args = WithdrawSolArgs {
        from_subaccount: None,
        amount: 1_000_000_000,
        address: VALID_ADDRESS.to_string(),
    };

    let info = build_consent_info(
        withdraw_sol_request(&args, Some(DisplayMessageType::GenericDisplay)),
        config(SolanaNetwork::Devnet),
    )
    .unwrap();
    let message = generic_display_message(info);
    assert!(message.starts_with("# Convert ckDevnetSOL to DevnetSOL"));
    assert!(message.contains("ckDevnetSOL minter"));
    assert!(message.contains("**Amount to convert:** `1 ckDevnetSOL`"));
    assert!(message.contains("**Withdrawal fee:** `0.001 ckDevnetSOL`"));
    assert!(message.contains("**Amount to receive:** `0.999 DevnetSOL`"));

    let info = build_consent_info(
        withdraw_sol_request(&args, Some(DisplayMessageType::FieldsDisplay)),
        config(SolanaNetwork::Devnet),
    )
    .unwrap();
    let fields_display = fields_display_message(info);
    assert_eq!(fields_display.intent, "ckDevnetSOL to DevnetSOL");
    assert_eq!(
        fields_display.fields,
        vec![
            (
                "Amount".to_string(),
                token_amount(1_000_000_000, "ckDevnetSOL")
            ),
            (
                "Withdrawal fee".to_string(),
                token_amount(WITHDRAWAL_FEE, "ckDevnetSOL")
            ),
            (
                "Amount received".to_string(),
                token_amount(1_000_000_000 - WITHDRAWAL_FEE, "DevnetSOL")
            ),
            ("DevnetSOL address".to_string(), text(VALID_ADDRESS)),
        ]
    );
}
