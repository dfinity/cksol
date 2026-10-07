use crate::{
    address::{DerivationPath, MINTER_DERIVATION_PATH, derivation_path},
    constants::FEE_PER_SIGNATURE,
    numeric::{LedgerBurnIndex, LedgerMintIndex},
    rpc::BlockHeight,
    state::DepositBalance,
};
use cksol_types::DepositSolId;
use cksol_types_internal::{InitArgs, UpgradeArgs};
use derive_more::From;
use ic_ed25519::PublicKey;
use ic_stable_structures::{Storable, storable::Bound};
use icrc_ledger_types::icrc1::account::Account;
use minicbor::{Decode, Encode};
use sol_rpc_types::Lamport;
use solana_address::Address;
use solana_hash::Hash;
use solana_message::Message;
use solana_sdk_ids::system_program;
use solana_signature::Signature;
use solana_system_interface::instruction::SystemInstruction;
use std::borrow::Cow;

/// A versioned Solana transaction message, allowing the minter to support
/// both legacy and versioned (v0) transactions in the future.
#[derive(Clone, Eq, PartialEq, Debug, Decode, Encode, From)]
pub enum VersionedMessage {
    #[n(0)]
    Legacy(
        #[n(0)]
        #[cbor(with = "cbor::message")]
        Message,
    ),
}

impl VersionedMessage {
    pub fn transaction_fee(&self) -> Lamport {
        let VersionedMessage::Legacy(message) = self;
        FEE_PER_SIGNATURE * message.header.num_required_signatures as u64
    }

    /// The nonce account advanced by the first instruction of the message,
    /// or `None` if the first instruction is not a system-program
    /// `AdvanceNonceAccount` instruction.
    pub fn advanced_nonce_account(&self) -> Option<Address> {
        let VersionedMessage::Legacy(message) = self;
        let instruction = message.instructions.first()?;
        let program_id = message
            .account_keys
            .get(usize::from(instruction.program_id_index))?;
        let is_advance_nonce_account = matches!(
            bincode::deserialize(&instruction.data),
            Ok(SystemInstruction::AdvanceNonceAccount)
        );
        if *program_id != system_program::ID || !is_advance_nonce_account {
            return None;
        }
        let nonce_account_index = instruction.accounts.first()?;
        message
            .account_keys
            .get(usize::from(*nonce_account_index))
            .copied()
    }
}

mod cbor;

#[derive(Eq, PartialEq, Debug, Decode, Encode)]
pub struct Event {
    /// The canister time at which the minter generated this event.
    #[n(0)]
    pub timestamp: u64,
    /// The event type.
    #[n(1)]
    pub payload: EventType,
}

#[derive(Clone, Eq, PartialEq, Debug, Decode, Encode)]
pub enum EventType {
    /// The minter initialization event.
    /// Must be the first event in the log.
    #[n(0)]
    Init(#[n(0)] InitArgs),
    /// The minter upgraded with the specified arguments.
    #[n(1)]
    Upgrade(#[n(0)] UpgradeArgs),
    /// The minter burned ckSOL for a withdrawal request.
    #[n(2)]
    AcceptedWithdrawalRequest(#[n(0)] WithdrawalRequest),
    /// The minter submitted a Solana transaction.
    #[n(3)]
    SubmittedTransaction {
        /// The transaction signature.
        #[cbor(n(0), with = "cbor::signature")]
        signature: Signature,
        /// The versioned transaction message.
        #[n(1)]
        message: VersionedMessage,
        /// The signers in signature order (fee payer first).
        #[n(2)]
        signers: Vec<Signer>,
        /// The purpose of this transaction, with what the minter needs to
        /// track it until it is finalized.
        #[n(3)]
        purpose: TransactionPurpose,
    },
    /// A previously submitted transaction was resubmitted with a new signature.
    /// The transaction message and signers remain the same.
    #[n(4)]
    ResubmittedTransaction {
        /// The signature of the old transaction being replaced
        #[cbor(n(0), with = "cbor::signature")]
        old_signature: Signature,
        /// The signature of the new transaction
        #[cbor(n(1), with = "cbor::signature")]
        new_signature: Signature,
        /// The block height of the new blockhash used in the resubmitted transaction.
        #[n(2)]
        new_block_height: BlockHeight,
    },
    /// A previously submitted Solana transaction has been finalized successfully.
    #[n(5)]
    SucceededTransaction {
        /// The signature of the succeeded Solana transaction.
        #[cbor(n(0), with = "cbor::signature")]
        signature: Signature,
    },
    /// A previously submitted Solana transaction has failed.
    #[n(6)]
    FailedTransaction {
        /// The signature of the failed Solana transaction.
        #[cbor(n(0), with = "cbor::signature")]
        signature: Signature,
    },
    /// A previously submitted Solana transaction has an expired blockhash
    /// and a null on-chain status, meaning it will never be executed.
    /// A withdrawal transaction is marked for resubmission;
    /// the deposits of a sweep transaction are dropped instead.
    #[n(7)]
    ExpiredTransaction {
        /// The signature of the expired Solana transaction.
        #[cbor(n(0), with = "cbor::signature")]
        signature: Signature,
    },
    /// A user queued the deposit address of an account for a sweep via `deposit_sol`.
    /// The deposit id is the next sequence number of the minter at the time of the event.
    #[n(8)]
    QueuedDeposit {
        #[n(0)]
        deposit_id: DepositSolId,
        #[n(1)]
        account: Account,
        #[cbor(n(2), with = "cbor::address")]
        address: Address,
        #[cbor(n(3), with = "cbor::deposit_balance")]
        balance: DepositBalance,
    },
    /// The minter read the amount that the finalized sweep transaction moved to its
    /// main account and enqueued a pending mint for each deposit of that sweep.
    #[n(9)]
    CreditedSweep {
        /// The signature of the finalized sweep transaction.
        #[cbor(n(0), with = "cbor::signature")]
        signature: Signature,
        /// The increase of the main account balance reported by the transaction metadata.
        #[n(1)]
        amount_received: Lamport,
        /// The mint enqueued for each deposit of the sweep.
        #[n(2)]
        mints: Vec<CreditedDeposit>,
    },
    /// The outcome of a finalized sweep transaction did not match the plan the minter
    /// submitted it with, so the amount to credit cannot be determined safely.
    ///
    /// The deposits are quarantined to avoid any double minting and will not be further
    /// processed without a minter upgrade.
    #[n(10)]
    QuarantinedSweep {
        /// The signature of the finalized sweep transaction.
        #[cbor(n(0), with = "cbor::signature")]
        signature: Signature,
    },
    /// The minter fetched its Schnorr Ed25519 master public key, from which
    /// its main address and all deposit addresses are derived.
    #[n(11)]
    MinterPublicKeyFetched {
        #[cbor(n(0), with = "cbor::ed25519_public_key")]
        public_key: PublicKey,
        #[cbor(n(1), with = "minicbor::bytes")]
        chain_code: [u8; 32],
    },
    /// The minter minted ckSOL on the ledger for a swept deposit whose sweep
    /// was credited.
    #[n(12)]
    MintedSweptDeposit {
        /// The identifier of the minted deposit.
        #[n(0)]
        deposit_id: DepositSolId,
        /// The mint transaction index on the ckSOL ledger.
        #[cbor(n(1), with = "cbor::id")]
        mint_block_index: LedgerMintIndex,
    },
    /// The pending mint of a swept deposit cannot be retried: either it became
    /// older than the 24-hour deduplication window of the ckSOL ledger, or the
    /// ledger definitively rejected it. Retrying the transfer with the same
    /// arguments fails forever, and fresh arguments could double mint.
    ///
    /// The deposit is quarantined to avoid any double minting and will not be
    /// further processed without manual intervention.
    ///
    /// If the minter was down past the deduplication window, the underlying
    /// transfer may nevertheless have landed on the ledger. Manual resolution
    /// must therefore first search the ledger for a mint whose memo carries the
    /// sweep signature before crediting by hand, otherwise a double mint
    /// results.
    #[n(13)]
    QuarantinedPendingMint {
        /// The identifier of the deposit whose pending mint was quarantined.
        #[n(0)]
        deposit_id: DepositSolId,
    },
    /// The minter bound a durable nonce account and its nonce value to the
    /// withdrawal requests of the given burn indices, before requesting the
    /// threshold signature. Since each burn index identifies a withdrawal
    /// request, the binding determines the transaction message, so that a
    /// signing failure leads to re-signing the identical message and never to
    /// a second message being signed for the same nonce value.
    #[n(14)]
    CreatedTransaction {
        /// The ledger burn indices of the withdrawal requests served by this transaction.
        #[cbor(n(0), with = "cbor::id_vec")]
        burn_indices: Vec<LedgerBurnIndex>,
        /// The durable nonce account bound to this transaction.
        #[cbor(n(1), with = "cbor::address")]
        nonce_account: Address,
        /// The nonce value the transaction carries in place of a recent blockhash.
        #[cbor(n(2), with = "cbor::hash")]
        nonce_value: Hash,
    },
}

/// The mint enqueued for one deposit of a `CreditedSweep` event.
#[derive(Clone, Copy, Eq, PartialEq, Debug, Decode, Encode)]
pub struct CreditedDeposit {
    #[n(0)]
    pub deposit_id: DepositSolId,
    /// The sweepable amount minus the deposit's share of the transaction fee of the sweep.
    #[n(1)]
    pub amount_to_mint: Lamport,
}

/// Payload of the `AcceptedWithdrawalRequest` event.
#[derive(Clone, Eq, PartialEq, Debug, Decode, Encode)]
pub struct WithdrawalRequest {
    /// The ledger account from which ckSOL was burned.
    #[n(0)]
    pub account: Account,
    /// The destination Solana address.
    #[cbor(n(1), with = "minicbor::bytes")]
    pub solana_address: [u8; 32],
    /// The burn transaction index on the ckSOL ledger.
    #[cbor(n(2), with = "cbor::id")]
    pub burn_block_index: LedgerBurnIndex,
    /// The total amount burned from the user (in lamports).
    #[n(3)]
    pub burned_amount: Lamport,
    /// The net amount to transfer to the user (in lamports).
    #[n(4)]
    pub amount_to_transfer: Lamport,
}

/// The key that produced one signature of a submitted Solana transaction.
#[derive(Clone, Eq, PartialEq, Debug, Decode, Encode)]
pub enum Signer {
    /// The minter itself, signing with the master key
    /// on [`MINTER_DERIVATION_PATH`].
    #[n(0)]
    Minter,
    /// A minter-controlled account, signing with the key derived
    /// for its deposit address.
    #[n(1)]
    Account(#[n(0)] Account),
}

impl Signer {
    pub fn derivation_path(&self) -> DerivationPath {
        match self {
            Signer::Minter => MINTER_DERIVATION_PATH,
            Signer::Account(account) => derivation_path(account),
        }
    }
}

/// The purpose of a submitted transaction, mirroring [`MinterTransaction`]:
/// each variant carries what the minter needs to track the transaction.
///
/// [`MinterTransaction`]: crate::state::MinterTransaction
#[derive(Clone, Eq, PartialEq, Debug, Decode, Encode)]
pub enum TransactionPurpose {
    /// Sweep the deposit addresses of deposits queued by `deposit_sol` into
    /// the minter's main account. The transaction uses a recent blockhash and
    /// is dropped once the blockhash expires.
    #[n(2)]
    SweepDeposit {
        /// The ids of the swept deposits.
        #[n(0)]
        deposit_ids: Vec<DepositSolId>,
        /// The block height of the block whose blockhash the transaction uses.
        /// The blockhash is valid for 150 blocks after that height.
        #[n(1)]
        block_height: BlockHeight,
    },
    /// Withdraw SOL to users' Solana addresses. The transaction uses a recent
    /// blockhash and is resubmitted once the blockhash expires.
    #[n(3)]
    Withdrawal {
        /// The ledger burn indices of the withdrawal requests included in this transaction.
        #[cbor(n(0), with = "cbor::id_vec")]
        burn_indices: Vec<LedgerBurnIndex>,
        /// The block height of the block whose blockhash the transaction uses.
        /// The blockhash is valid for 150 blocks after that height.
        #[n(1)]
        block_height: BlockHeight,
    },
    /// Withdraw SOL to users' Solana addresses. The transaction carries the
    /// nonce value of a durable nonce account instead of a recent blockhash,
    /// so it never expires.
    #[n(4)]
    NonceWithdrawal {
        /// The ledger burn indices of the withdrawal requests included in this transaction.
        #[cbor(n(0), with = "cbor::id_vec")]
        burn_indices: Vec<LedgerBurnIndex>,
    },
}

impl Storable for Event {
    fn to_bytes(&self) -> Cow<'_, [u8]> {
        let mut buf = vec![];
        minicbor::encode(self, &mut buf).expect("event encoding should always succeed");
        Cow::Owned(buf)
    }

    fn from_bytes(bytes: Cow<[u8]>) -> Self {
        minicbor::decode(bytes.as_ref())
            .unwrap_or_else(|e| panic!("failed to decode event bytes {}: {e}", hex::encode(bytes)))
    }

    const BOUND: Bound = Bound::Unbounded;
}
