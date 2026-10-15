#[cfg(test)]
mod tests;

use crate::{
    numeric::{LedgerBurnIndex, LedgerMintIndex},
    rpc::BlockHeight,
    state::{
        DepositBalance,
        event::{
            CreditedDeposit, Event, EventType, Signer, TransactionPurpose, VersionedMessage,
            WithdrawalRequest,
        },
    },
};
use cksol_types_internal::event::{
    CreditedDeposit as CandidCreditedDeposit, Event as CandidEvent, EventType as CandidEventType,
    Signer as CandidSigner, TransactionPurpose as CandidTransactionPurpose,
    VersionedTransactionMessage,
};
use ic_ed25519::PublicKey;
use sol_rpc_types::{
    Hash as CandidHash, Lamport, Pubkey as CandidAddress, Signature as CandidSignature,
};
use solana_address::Address;
use solana_hash::Hash;
use solana_signature::Signature;
use thiserror::Error;

impl From<Event> for CandidEvent {
    fn from(event: Event) -> Self {
        CandidEvent {
            timestamp: event.timestamp,
            payload: CandidEventType::from(event.payload),
        }
    }
}

impl From<EventType> for CandidEventType {
    fn from(event_type: EventType) -> Self {
        match event_type {
            EventType::Init(args) => CandidEventType::Init(args),
            EventType::Upgrade(args) => CandidEventType::Upgrade(args),
            EventType::AcceptedWithdrawalRequest(request) => {
                CandidEventType::AcceptedWithdrawalRequest {
                    account: request.account,
                    solana_address: CandidAddress::from(Address::from(request.solana_address)),
                    burn_block_index: *request.burn_block_index.get(),
                    burned_amount: request.burned_amount,
                    amount_to_transfer: request.amount_to_transfer,
                }
            }
            EventType::SubmittedTransaction {
                signature,
                message,
                signers,
                purpose,
            } => CandidEventType::SubmittedTransaction {
                signature: CandidSignature::from(signature),
                transaction: VersionedTransactionMessage::from(message),
                signers: signers.into_iter().map(CandidSigner::from).collect(),
                purpose: CandidTransactionPurpose::from(purpose),
            },
            EventType::SucceededTransaction { signature } => {
                CandidEventType::SucceededTransaction {
                    signature: CandidSignature::from(signature),
                }
            }
            EventType::FailedTransaction { signature } => CandidEventType::FailedTransaction {
                signature: CandidSignature::from(signature),
            },
            EventType::ExpiredTransaction { signature } => CandidEventType::ExpiredTransaction {
                signature: CandidSignature::from(signature),
            },
            EventType::QueuedDeposit {
                deposit_id,
                account,
                address,
                balance,
                automatic_deposit_fee,
            } => CandidEventType::QueuedDeposit {
                deposit_id,
                account,
                address: CandidAddress::from(address),
                balance: Lamport::from(balance),
                automatic_deposit_fee,
            },
            EventType::CreditedSweep {
                signature,
                amount_received,
                mints,
            } => CandidEventType::CreditedSweep {
                signature: CandidSignature::from(signature),
                amount_received,
                mints: mints.into_iter().map(CandidCreditedDeposit::from).collect(),
            },
            EventType::QuarantinedSweep { signature } => CandidEventType::QuarantinedSweep {
                signature: CandidSignature::from(signature),
            },
            EventType::MinterPublicKeyFetched {
                public_key,
                chain_code,
            } => CandidEventType::MinterPublicKeyFetched {
                public_key: public_key.serialize_raw().to_vec(),
                chain_code: chain_code.to_vec(),
            },
            EventType::MintedSweptDeposit {
                deposit_id,
                mint_block_index,
            } => CandidEventType::MintedSweptDeposit {
                deposit_id,
                mint_block_index: *mint_block_index.get(),
            },
            EventType::QuarantinedPendingMint { deposit_id } => {
                CandidEventType::QuarantinedPendingMint { deposit_id }
            }
            EventType::CreatedWithdrawalTransaction {
                burn_indices,
                nonce_account,
                nonce_value,
            } => CandidEventType::CreatedWithdrawalTransaction {
                burn_indices: burn_indices.iter().map(|idx| *idx.get()).collect(),
                nonce_account: CandidAddress::from(nonce_account),
                nonce_value: CandidHash::from(nonce_value),
            },
        }
    }
}

impl From<VersionedMessage> for VersionedTransactionMessage {
    fn from(message: VersionedMessage) -> Self {
        match message {
            VersionedMessage::Legacy(message) => VersionedTransactionMessage::Legacy(
                bincode::serialize(&message).expect("serializing transaction should succeed"),
            ),
        }
    }
}

impl From<Signer> for CandidSigner {
    fn from(signer: Signer) -> Self {
        match signer {
            Signer::Minter => CandidSigner::Minter,
            Signer::Account(account) => CandidSigner::Account(account),
        }
    }
}

impl From<TransactionPurpose> for CandidTransactionPurpose {
    fn from(purpose: TransactionPurpose) -> Self {
        match purpose {
            TransactionPurpose::SweepDeposit {
                deposit_ids,
                block_height,
            } => CandidTransactionPurpose::SweepDeposit {
                deposit_ids,
                block_height: block_height.get(),
            },
            TransactionPurpose::Withdrawal { burn_indices } => {
                CandidTransactionPurpose::Withdrawal {
                    burn_indices: burn_indices.iter().map(|idx| *idx.get()).collect(),
                }
            }
        }
    }
}

impl From<CreditedDeposit> for CandidCreditedDeposit {
    fn from(mint: CreditedDeposit) -> Self {
        CandidCreditedDeposit {
            deposit_id: mint.deposit_id,
            amount_to_mint: mint.amount_to_mint,
        }
    }
}

impl TryFrom<CandidEvent> for Event {
    type Error = InvalidCandidEvent;

    fn try_from(event: CandidEvent) -> Result<Self, Self::Error> {
        Ok(Event {
            timestamp: event.timestamp,
            payload: EventType::try_from(event.payload)?,
        })
    }
}

impl TryFrom<CandidEventType> for EventType {
    type Error = InvalidCandidEvent;

    fn try_from(event_type: CandidEventType) -> Result<Self, Self::Error> {
        Ok(match event_type {
            CandidEventType::Init(args) => EventType::Init(args),
            CandidEventType::Upgrade(args) => EventType::Upgrade(args),
            CandidEventType::AcceptedWithdrawalRequest {
                account,
                solana_address,
                burn_block_index,
                burned_amount,
                amount_to_transfer,
            } => EventType::AcceptedWithdrawalRequest(WithdrawalRequest {
                account,
                solana_address: Address::from(solana_address).to_bytes(),
                burn_block_index: LedgerBurnIndex::new(burn_block_index),
                burned_amount,
                amount_to_transfer,
            }),
            CandidEventType::SubmittedTransaction {
                signature,
                transaction,
                signers,
                purpose,
            } => EventType::SubmittedTransaction {
                signature: Signature::from(signature),
                message: VersionedMessage::try_from(transaction)?,
                signers: signers.into_iter().map(Signer::from).collect(),
                purpose: TransactionPurpose::from(purpose),
            },
            CandidEventType::SucceededTransaction { signature } => {
                EventType::SucceededTransaction {
                    signature: Signature::from(signature),
                }
            }
            CandidEventType::FailedTransaction { signature } => EventType::FailedTransaction {
                signature: Signature::from(signature),
            },
            CandidEventType::ExpiredTransaction { signature } => EventType::ExpiredTransaction {
                signature: Signature::from(signature),
            },
            CandidEventType::QueuedDeposit {
                deposit_id,
                account,
                address,
                balance,
                automatic_deposit_fee,
            } => EventType::QueuedDeposit {
                deposit_id,
                account,
                address: Address::from(address),
                balance: DepositBalance::new(balance).ok_or(
                    InvalidCandidEvent::DepositBalanceBelowRentExemption(balance),
                )?,
                automatic_deposit_fee,
            },
            CandidEventType::CreditedSweep {
                signature,
                amount_received,
                mints,
            } => EventType::CreditedSweep {
                signature: Signature::from(signature),
                amount_received,
                mints: mints.into_iter().map(CreditedDeposit::from).collect(),
            },
            CandidEventType::QuarantinedSweep { signature } => EventType::QuarantinedSweep {
                signature: Signature::from(signature),
            },
            CandidEventType::MinterPublicKeyFetched {
                public_key,
                chain_code,
            } => EventType::MinterPublicKeyFetched {
                public_key: PublicKey::deserialize_raw(&public_key)
                    .map_err(|e| InvalidCandidEvent::InvalidPublicKey(e.to_string()))?,
                chain_code: <[u8; 32]>::try_from(chain_code)
                    .map_err(|bytes| InvalidCandidEvent::InvalidChainCode(bytes.len()))?,
            },
            CandidEventType::MintedSweptDeposit {
                deposit_id,
                mint_block_index,
            } => EventType::MintedSweptDeposit {
                deposit_id,
                mint_block_index: LedgerMintIndex::new(mint_block_index),
            },
            CandidEventType::QuarantinedPendingMint { deposit_id } => {
                EventType::QuarantinedPendingMint { deposit_id }
            }
            CandidEventType::CreatedWithdrawalTransaction {
                burn_indices,
                nonce_account,
                nonce_value,
            } => EventType::CreatedWithdrawalTransaction {
                burn_indices: burn_indices.into_iter().map(LedgerBurnIndex::new).collect(),
                nonce_account: Address::from(nonce_account),
                nonce_value: Hash::from(nonce_value),
            },
        })
    }
}

impl TryFrom<VersionedTransactionMessage> for VersionedMessage {
    type Error = InvalidCandidEvent;

    fn try_from(message: VersionedTransactionMessage) -> Result<Self, Self::Error> {
        match message {
            VersionedTransactionMessage::Legacy(bytes) => bincode::deserialize(&bytes)
                .map(VersionedMessage::Legacy)
                .map_err(|e| InvalidCandidEvent::InvalidMessage(e.to_string())),
        }
    }
}

impl From<CandidSigner> for Signer {
    fn from(signer: CandidSigner) -> Self {
        match signer {
            CandidSigner::Minter => Signer::Minter,
            CandidSigner::Account(account) => Signer::Account(account),
        }
    }
}

impl From<CandidTransactionPurpose> for TransactionPurpose {
    fn from(purpose: CandidTransactionPurpose) -> Self {
        match purpose {
            CandidTransactionPurpose::SweepDeposit {
                deposit_ids,
                block_height,
            } => TransactionPurpose::SweepDeposit {
                deposit_ids,
                block_height: BlockHeight::new(block_height),
            },
            CandidTransactionPurpose::Withdrawal { burn_indices } => {
                TransactionPurpose::Withdrawal {
                    burn_indices: burn_indices.into_iter().map(LedgerBurnIndex::new).collect(),
                }
            }
        }
    }
}

impl From<CandidCreditedDeposit> for CreditedDeposit {
    fn from(mint: CandidCreditedDeposit) -> Self {
        CreditedDeposit {
            deposit_id: mint.deposit_id,
            amount_to_mint: mint.amount_to_mint,
        }
    }
}

/// A Candid event that cannot be converted back into a minter event.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum InvalidCandidEvent {
    #[error("invalid transaction message: {0}")]
    InvalidMessage(String),
    #[error("invalid public key: {0}")]
    InvalidPublicKey(String),
    #[error("invalid chain code: expected 32 bytes, got {0}")]
    InvalidChainCode(usize),
    #[error("deposit balance of {0} lamports is below the rent exemption threshold")]
    DepositBalanceBelowRentExemption(Lamport),
}
