use cksol_types::{GetTransactionError, InvalidTransaction};
use sol_rpc_types::{GetTransactionEncoding, Lamport, MultiRpcResult, Signature};
use solana_address::Address;
use solana_message::compiled_instruction::CompiledInstruction;
use solana_system_interface::instruction::SystemInstruction;
use solana_transaction_status_client_types::{
    option_serializer::OptionSerializer, EncodedConfirmedTransactionWithStatusMeta, UiInstruction,
};

pub async fn try_get_transaction(
    signature: Signature,
) -> Result<Option<EncodedConfirmedTransactionWithStatusMeta>, GetTransactionError> {
    let signature = solana_signature::Signature::from(signature);

    let client = sol_rpc_client::SolRpcClient::builder_for_ic().build();

    let result = client
        .get_transaction(signature)
        .with_encoding(GetTransactionEncoding::Base64)
        .with_cycles(10_000_000_000_000)
        .try_send()
        .await;

    match result {
        Ok(MultiRpcResult::Consistent(Ok(maybe_tx))) => Ok(maybe_tx),
        Ok(MultiRpcResult::Consistent(Err(e))) => Err(GetTransactionError::RpcError(e)),
        Ok(MultiRpcResult::Inconsistent(results)) => Err(GetTransactionError::InconsistentResults(
            results
                .into_iter()
                .map(|(source, result)| {
                    (
                        source,
                        result.and_then(|maybe_tx|
                            maybe_tx.map(sol_rpc_types::EncodedConfirmedTransactionWithStatusMeta::try_from).transpose()
                        ),
                    )
                })
                .collect(),
        )),
        Err(e) => Err(GetTransactionError::IcError(e.to_string())),
    }
}

pub fn extract_deposits_to_address(
    transaction: EncodedConfirmedTransactionWithStatusMeta,
    destination: Address,
) -> Result<Vec<(InstructionIndex, SolTransfer)>, InvalidTransaction> {
    let mut transfers = Vec::new();

    let message = transaction
        .transaction
        .transaction
        .decode()
        .ok_or(InvalidTransaction::DecodingFailed)?
        .message;
    let account_keys = message.static_account_keys();

    // Look at top-level instructions
    for (index, instruction) in message.instructions().iter().enumerate() {
        if let Some(transfer) = try_as_deposit_to(account_keys, instruction, &destination)? {
            ic_cdk::println!(
                "Found deposit to {:?} from {:?} at instruction {:?}: {:?} lamports",
                transfer.from,
                transfer.to,
                index,
                transfer.amount,
            );
            transfers.push((
                InstructionIndex {
                    index,
                    inner_index: None,
                },
                transfer,
            ));
        }
    }

    // Get the transaction inner instructions
    let inner_instructions = transaction
        .transaction
        .meta
        .into_iter()
        .flat_map(|meta| match meta.inner_instructions {
            OptionSerializer::Some(inner_instructions) => Some(inner_instructions.into_iter()),
            _ => None,
        })
        .flatten()
        .flat_map(|inner_instruction| {
            // Index of the top-level instruction containing this inner instruction
            let index = inner_instruction.index as usize;
            inner_instruction
                .instructions
                .into_iter()
                .enumerate()
                .filter_map(
                    move |(inner_index, inner_instruction)| match inner_instruction {
                        UiInstruction::Compiled(inner_instruction) => {
                            let index = InstructionIndex {
                                index,
                                inner_index: Some(inner_index),
                            };
                            Some((index, inner_instruction))
                        }
                        // This should not happen when calling `getTransaction` with base64 encoding
                        UiInstruction::Parsed(_) => None,
                    },
                )
        });

    for (index, inner_instruction) in inner_instructions {
        let inner_instruction = CompiledInstruction {
            program_id_index: inner_instruction.program_id_index,
            accounts: inner_instruction.accounts,
            data: bs58::decode(inner_instruction.data)
                .into_vec()
                .map_err(|_| InvalidTransaction::DecodingFailed)?,
        };
        if let Some(transfer) = try_as_deposit_to(account_keys, &inner_instruction, &destination)? {
            ic_cdk::println!(
                "Found deposit to {:?} from {:?} at instruction {:?} (inner instruction {:?}): {:?} lamports",
                transfer.from,
                transfer.to,
                index.index,
                index.inner_index.unwrap(),
                transfer.amount,
            );
            transfers.push((index, transfer));
        }
    }

    Ok(transfers)
}

fn try_as_deposit_to(
    account_keys: &[Address],
    instruction: &CompiledInstruction,
    destination: &Address,
) -> Result<Option<SolTransfer>, InvalidTransaction> {
    let program_id = account_keys
        .get(instruction.program_id_index as usize)
        .ok_or(InvalidTransaction::DecodingFailed)?;
    if program_id != &solana_system_interface::program::id() {
        return Ok(None);
    }

    let amount = match bincode::deserialize::<SystemInstruction>(&instruction.data) {
        Ok(SystemInstruction::Transfer { lamports }) => lamports,
        _ => return Ok(None),
    };

    let account_index = instruction
        .accounts
        .first()
        .ok_or(InvalidTransaction::DecodingFailed)?;
    let address = account_keys
        .get(*account_index as usize)
        .ok_or(InvalidTransaction::DecodingFailed)?;
    let from = address;

    let account_index = instruction
        .accounts
        .get(1)
        .ok_or(InvalidTransaction::DecodingFailed)?;
    let address = account_keys
        .get(*account_index as usize)
        .ok_or(InvalidTransaction::DecodingFailed)?;
    let to = address;

    if to != destination {
        return Ok(None);
    }

    Ok(Some(SolTransfer {
        from: *from,
        to: *to,
        amount,
    }))
}

pub struct InstructionIndex {
    pub index: usize,
    pub inner_index: Option<usize>,
}

pub struct SolTransfer {
    pub from: Address,
    pub to: Address,
    pub amount: Lamport,
}
