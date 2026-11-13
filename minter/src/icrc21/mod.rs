//! Implementation of the [ICRC-21](https://github.com/dfinity/ICRC/blob/main/ICRCs/ICRC-21/ICRC-21.md)
//! Canister Call Consent Message standard for the ckSOL minter.

use crate::{dashboard::lamports_to_sol, state::read_state, withdraw::validate_withdrawal_request};
use candid::{DecoderConfig, decode_one_with_config};
use cksol_types::{StandardRecord, WithdrawSolArgs, WithdrawSolError};
use cksol_types_internal::SolanaNetwork;
use icrc_ledger_types::icrc21::{
    errors::{ErrorInfo, Icrc21Error},
    lib::MAX_CONSENT_MESSAGE_ARG_SIZE_BYTES,
    requests::{ConsentMessageMetadata, ConsentMessageRequest, DisplayMessageType},
    responses::{ConsentInfo, ConsentMessage, FieldsDisplay, Value},
};
use solana_address::Address;

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
            // Check the amount and the destination as `withdraw_sol` does, so that the
            // consent message only describes requests the minter accepts. The other
            // checks of `withdraw_sol` are skipped: the caller of this endpoint is not
            // necessarily the principal that signs the withdrawal, so the anonymous
            // caller check does not apply, and an empty nonce pool only makes the
            // withdrawal temporarily unavailable. Parsing the address also rules out
            // Markdown injection in the GenericDisplay output (e.g. an "address" with
            // newlines or backticks crafted to fake additional fields).
            let address = validate_withdrawal_request(args.amount, &args.address)
                .map_err(withdrawal_request_error)?;
            build_withdraw_sol_message(&args, address, &display_type)
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
) -> ConsentMessage {
    let (solana_network, withdrawal_fee) = read_state(|s| (s.solana_network(), s.withdrawal_fee()));
    let TokenSymbols { cksol, sol } = TokenSymbols::for_network(solana_network);
    // Same computation as in `withdraw`, with the current withdrawal fee: an
    // upgrade can change the fee before the call executes. The amount is at least
    // the minimum withdrawal amount, which the state ensures exceeds the fee.
    let estimated_amount_to_transfer = args
        .amount
        .checked_sub(withdrawal_fee)
        .expect("BUG: withdrawal amount must be >= withdrawal fee");
    match display_type {
        DisplayMessageType::GenericDisplay => {
            let mut message = format!(
                "# Convert {cksol} to {sol}\n\n\
                 Authorize the {cksol} minter to burn {cksol} from your account and \
                 send the equivalent amount in {sol}, minus the withdrawal fee in effect \
                 when the withdrawal is executed, to the Solana address below.\n\n\
                 **Amount to convert:** `{amount} {cksol}`\n\n\
                 **Current withdrawal fee:** `{fee} {cksol}`\n\n\
                 **Estimated amount to receive:** `{received} {sol}`\n\n\
                 **Solana destination address:**\n`{address}`",
                amount = lamports_to_sol(args.amount),
                fee = lamports_to_sol(withdrawal_fee),
                received = lamports_to_sol(estimated_amount_to_transfer),
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
                    "Current fee".to_string(),
                    Value::TokenAmount {
                        decimals: DECIMALS,
                        amount: withdrawal_fee,
                        symbol: cksol.to_string(),
                    },
                ),
                (
                    "Est. amount received".to_string(),
                    Value::TokenAmount {
                        decimals: DECIMALS,
                        amount: estimated_amount_to_transfer,
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

fn withdrawal_request_error(error: WithdrawSolError) -> Icrc21Error {
    let unsupported = |description| Icrc21Error::UnsupportedCanisterCall(ErrorInfo { description });
    match error {
        WithdrawSolError::ValueTooSmall {
            minimum_withdrawal_amount,
            withdrawal_amount,
        } => unsupported(format!(
            "The withdrawal amount {withdrawal_amount} is below the minimum withdrawal amount \
             {minimum_withdrawal_amount} (in lamports)."
        )),
        WithdrawSolError::MalformedAddress(reason)
        | WithdrawSolError::InvalidDestination(reason) => {
            unsupported(format!("Invalid Solana destination address: {reason}"))
        }
        WithdrawSolError::TemporarilyUnavailable(reason) => {
            Icrc21Error::ConsentMessageUnavailable(ErrorInfo {
                description: reason,
            })
        }
        error => unsupported(format!("Invalid withdrawal request: {error:?}")),
    }
}
