//! Implementation of the [ICRC-21](https://github.com/dfinity/ICRC/blob/main/ICRCs/ICRC-21/ICRC-21.md)
//! Canister Call Consent Message standard for the ckSOL minter.

use crate::state::{State, read_state};
use candid::{DecoderConfig, decode_one_with_config};
use cksol_types::{StandardRecord, WithdrawSolArgs};
use cksol_types_internal::SolanaNetwork;
use icrc_ledger_types::icrc21::{
    errors::{ErrorInfo, Icrc21Error},
    lib::MAX_CONSENT_MESSAGE_ARG_SIZE_BYTES,
    requests::{ConsentMessageMetadata, ConsentMessageRequest, DisplayMessageType},
    responses::{ConsentInfo, ConsentMessage, FieldsDisplay, Value},
};
use sol_rpc_types::Lamport;
use solana_address::Address;
use std::str::FromStr;

#[cfg(test)]
mod tests;

/// The number of decimals used to display token amounts.
/// Both ckSOL and SOL use 9 decimals (1 SOL = 10^9 lamports).
const DECIMALS: u8 = 9;

/// Token symbols used in consent messages. They depend on the configured
/// Solana network so that test deployments use the test-token names.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
struct TokenSymbols {
    /// The ledger token, e.g. "ckSOL" on Mainnet, "ckDevnetSOL" on Devnet.
    cksol: &'static str,
    /// The native Solana token, e.g. "SOL" on Mainnet, "DevnetSOL" on Devnet.
    sol: &'static str,
}

impl TokenSymbols {
    fn for_network(network: SolanaNetwork) -> Self {
        match network {
            SolanaNetwork::Mainnet => Self {
                cksol: "ckSOL",
                sol: "SOL",
            },
            SolanaNetwork::Devnet => Self {
                cksol: "ckDevnetSOL",
                sol: "DevnetSOL",
            },
            SolanaNetwork::Testnet => Self {
                cksol: "ckTestnetSOL",
                sol: "TestnetSOL",
            },
        }
    }
}

/// The minter parameters that determine the content of a consent message.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
struct ConsentMessageConfig {
    solana_network: SolanaNetwork,
    withdrawal_fee: Lamport,
    minimum_withdrawal_amount: Lamport,
}

impl From<&State> for ConsentMessageConfig {
    fn from(state: &State) -> Self {
        Self {
            solana_network: state.solana_network(),
            withdrawal_fee: state.withdrawal_fee(),
            minimum_withdrawal_amount: state.minimum_withdrawal_amount(),
        }
    }
}

pub fn icrc10_supported_standards() -> Vec<StandardRecord> {
    vec![
        StandardRecord {
            name: "ICRC-10".to_string(),
            url: "https://github.com/dfinity/ICRC/blob/main/ICRCs/ICRC-10/ICRC-10.md".to_string(),
        },
        StandardRecord {
            name: "ICRC-21".to_string(),
            url: "https://github.com/dfinity/ICRC/blob/main/ICRCs/ICRC-21/ICRC-21.md".to_string(),
        },
    ]
}

pub fn icrc21_canister_call_consent_message(
    consent_msg_request: ConsentMessageRequest,
) -> Result<ConsentInfo, Icrc21Error> {
    let config = read_state(|state| ConsentMessageConfig::from(state));
    build_consent_info(consent_msg_request, config)
}

fn build_consent_info(
    consent_msg_request: ConsentMessageRequest,
    config: ConsentMessageConfig,
) -> Result<ConsentInfo, Icrc21Error> {
    if consent_msg_request.arg.len() > MAX_CONSENT_MESSAGE_ARG_SIZE_BYTES as usize {
        return Err(Icrc21Error::UnsupportedCanisterCall(ErrorInfo {
            description: format!(
                "The argument size is too large. The maximum allowed size is \
                 {MAX_CONSENT_MESSAGE_ARG_SIZE_BYTES} bytes."
            ),
        }));
    }

    let display_type = consent_msg_request
        .user_preferences
        .device_spec
        .clone()
        .unwrap_or(DisplayMessageType::GenericDisplay);

    let consent_message = match consent_msg_request.method.as_str() {
        "withdraw_sol" => {
            let args = decode_one_with_config::<WithdrawSolArgs>(
                consent_msg_request.arg.as_slice(),
                &decoder_config(),
            )
            .map_err(|e| {
                Icrc21Error::UnsupportedCanisterCall(ErrorInfo {
                    description: format!("Failed to decode WithdrawSolArgs: {e}"),
                })
            })?;
            let address = parse_address(&args.address)?;
            validate_amount(args.amount, config.minimum_withdrawal_amount)?;
            build_withdraw_sol_message(&args, address, &display_type, config)
        }
        method => {
            return Err(Icrc21Error::UnsupportedCanisterCall(ErrorInfo {
                description: format!(
                    "The method '{method}' is not supported by the ckSOL minter ICRC-21 endpoint."
                ),
            }));
        }
    };

    // Respond in English regardless of what the client requested for now.
    let metadata = ConsentMessageMetadata {
        language: "en".to_string(),
        utc_offset_minutes: consent_msg_request
            .user_preferences
            .metadata
            .utc_offset_minutes,
    };

    Ok(ConsentInfo {
        metadata,
        consent_message,
    })
}

fn build_withdraw_sol_message(
    args: &WithdrawSolArgs,
    address: Address,
    display_type: &DisplayMessageType,
    config: ConsentMessageConfig,
) -> ConsentMessage {
    let TokenSymbols { cksol, sol } = TokenSymbols::for_network(config.solana_network);
    // Same computation as in `withdraw`: the withdrawal fee is deducted from the
    // burned amount, which `validate_amount` ensures is at least the minimum
    // withdrawal amount and hence larger than the fee.
    let amount_to_transfer = args
        .amount
        .checked_sub(config.withdrawal_fee)
        .expect("BUG: withdrawal amount must be >= withdrawal fee");
    match display_type {
        DisplayMessageType::GenericDisplay => {
            let mut message = format!(
                "# Convert {cksol} to {sol}\n\n\
                 Authorize the {cksol} minter to burn {cksol} from your account and \
                 send the equivalent amount in {sol}, minus the withdrawal fee, to \
                 the Solana address below.\n\n\
                 **Amount to convert:** `{amount} {cksol}`\n\n\
                 **Withdrawal fee:** `{fee} {cksol}`\n\n\
                 **Amount to receive:** `{received} {sol}`\n\n\
                 **Solana destination address:**\n`{address}`",
                amount = format_amount(args.amount, DECIMALS),
                fee = format_amount(config.withdrawal_fee, DECIMALS),
                received = format_amount(amount_to_transfer, DECIMALS),
            );
            if let Some(subaccount) = args.from_subaccount {
                message.push_str(&format!(
                    "\n\n**{cksol} source subaccount:**\n`{}`",
                    hex::encode(subaccount)
                ));
            }
            ConsentMessage::GenericDisplayMessage(message)
        }
        DisplayMessageType::FieldsDisplay => {
            // Long values (Solana addresses, subaccount hex) are sent as a
            // single `Value::Text` per the ICRC-21 spec — wallets are
            // responsible for paginating them across screens.
            let mut fields = vec![
                (
                    "Amount".to_string(),
                    Value::TokenAmount {
                        decimals: DECIMALS,
                        amount: args.amount,
                        symbol: cksol.to_string(),
                    },
                ),
                (
                    "Withdrawal fee".to_string(),
                    Value::TokenAmount {
                        decimals: DECIMALS,
                        amount: config.withdrawal_fee,
                        symbol: cksol.to_string(),
                    },
                ),
                (
                    "Amount received".to_string(),
                    Value::TokenAmount {
                        decimals: DECIMALS,
                        amount: amount_to_transfer,
                        symbol: sol.to_string(),
                    },
                ),
                (
                    format!("{sol} address"),
                    Value::Text {
                        content: address.to_string(),
                    },
                ),
            ];
            if let Some(subaccount) = args.from_subaccount {
                fields.push((
                    "From subaccount".to_string(),
                    Value::Text {
                        content: hex::encode(subaccount),
                    },
                ));
            }
            ConsentMessage::FieldsDisplayMessage(FieldsDisplay {
                intent: format!("{cksol} to {sol}"),
                fields,
            })
        }
    }
}

/// The Candid decoder configuration for the arguments of the call to describe.
///
/// The argument size limit alone does not bound the decoding work, since a few
/// bytes can declare a vector of billions of `null` values. Use the same
/// skipping quota as the `ic_cdk` endpoint macros apply to the arguments of
/// `withdraw_sol` itself.
fn decoder_config() -> DecoderConfig {
    let mut config = DecoderConfig::new();
    config.set_skipping_quota(10_000);
    config
}

/// Parses `address` as a Solana address before it gets interpolated into a
/// consent message. This both guarantees the user is shown a meaningful
/// (parseable) destination and rules out Markdown-injection vectors in the
/// GenericDisplay output (e.g. an "address" that contains newlines or backticks
/// crafted to fake additional fields). Uses the same parser as `withdraw_sol`,
/// so any address the consent endpoint accepts is also accepted by the actual
/// call.
fn parse_address(address: &str) -> Result<Address, Icrc21Error> {
    Address::from_str(address).map_err(|e| {
        Icrc21Error::UnsupportedCanisterCall(ErrorInfo {
            description: format!("Invalid Solana destination address: {e}"),
        })
    })
}

/// Rejects a withdrawal amount below the minimum withdrawal amount, as
/// `withdraw_sol` does, so that the amount to receive shown to the user is
/// never negative.
fn validate_amount(amount: Lamport, minimum_withdrawal_amount: Lamport) -> Result<(), Icrc21Error> {
    if amount < minimum_withdrawal_amount {
        return Err(Icrc21Error::UnsupportedCanisterCall(ErrorInfo {
            description: format!(
                "The withdrawal amount {amount} is below the minimum withdrawal amount \
                 {minimum_withdrawal_amount} (in lamports)."
            ),
        }));
    }
    Ok(())
}

fn format_amount(amount: u64, decimals: u8) -> String {
    let divisor = 10_u64.pow(decimals as u32);
    let whole = amount / divisor;
    let frac = amount % divisor;
    if frac == 0 {
        format!("{whole}")
    } else {
        let frac_str = format!("{frac:0width$}", width = decimals as usize);
        let trimmed = frac_str.trim_end_matches('0');
        format!("{whole}.{trimmed}")
    }
}
