use crate::state::event::{
    CreditedDeposit, Event, EventType, Signer, TransactionPurpose, VersionedMessage,
};
use cksol_types_internal::event::{
    CreditedDeposit as CandidCreditedDeposit, Event as CandidEvent, EventType as CandidEventType,
    Signer as CandidSigner, TransactionPurpose as CandidTransactionPurpose,
    VersionedTransactionMessage,
};
use solana_address::Address;

impl From<Event> for CandidEvent {
    fn from(event: Event) -> Self {
        CandidEvent {
            timestamp: event.timestamp,
            payload: event.payload.into(),
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
                    solana_address: Address::from(request.solana_address).into(),
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
                signature: signature.into(),
                transaction: message.into(),
                signers: signers.into_iter().map(CandidSigner::from).collect(),
                purpose: purpose.into(),
            },
            EventType::SucceededTransaction { signature } => {
                CandidEventType::SucceededTransaction {
                    signature: signature.into(),
                }
            }
            EventType::FailedTransaction { signature } => CandidEventType::FailedTransaction {
                signature: signature.into(),
            },
            EventType::ExpiredTransaction { signature } => CandidEventType::ExpiredTransaction {
                signature: signature.into(),
            },
            EventType::QueuedDeposit {
                deposit_id,
                account,
                address,
                balance,
            } => CandidEventType::QueuedDeposit {
                deposit_id,
                account,
                address: address.into(),
                balance: balance.into(),
            },
            EventType::CreditedSweep {
                signature,
                amount_received,
                mints,
            } => CandidEventType::CreditedSweep {
                signature: signature.into(),
                amount_received,
                mints: mints.into_iter().map(CandidCreditedDeposit::from).collect(),
            },
            EventType::QuarantinedSweep { signature } => CandidEventType::QuarantinedSweep {
                signature: signature.into(),
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
                nonce_account: nonce_account.into(),
                nonce_value: nonce_value.into(),
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
