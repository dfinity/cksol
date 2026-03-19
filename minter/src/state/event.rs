use crate::numeric::{LedgerBurnIndex, LedgerMintIndex};
use cksol_types_internal::{InitArgs, UpgradeArgs};
use ic_stable_structures::{Storable, storable::Bound};
use icrc_ledger_types::icrc1::account::Account;
use minicbor::{Decode, Encode};
use sol_rpc_types::Lamport;
use solana_message::Message;
use solana_signature::Signature;
use std::borrow::Cow;

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
    /// The minter discovered a Solana transaction that is a valid ckSOL
    /// deposit for the given account. ckSOL tokens have not yet been
    /// minted for this deposit.
    #[n(2)]
    AcceptedDeposit {
        #[n(0)]
        deposit_id: DepositId,
        #[n(1)]
        deposit_amount: Lamport,
        #[n(2)]
        amount_to_mint: Lamport,
    },
    /// The minter discovered a Solana transaction that is a valid ckSOL
    /// deposit, but it is unknown whether ckSOL tokens were minted for
    /// it or not, most likely because there was an unexpected panic in
    /// the callback.
    ///
    /// The deposit is quarantined to avoid any double minting and
    /// will not be further processed without manual intervention.
    #[n(3)]
    QuarantinedDeposit(#[n(0)] DepositId),
    #[n(4)]
    Minted {
        #[n(0)]
        deposit_id: DepositId,
        #[cbor(n(1), with = "cbor::id")]
        mint_block_index: LedgerMintIndex,
    },
    /// The minter burned ckSOL for a withdrawal request.
    #[n(5)]
    AcceptedWithdrawSolRequest(#[n(0)] WithdrawSolRequest),
    /// Submitted a Solana transaction
    #[n(6)]
    SubmittedTransaction {
        /// The transaction signature
        #[cbor(n(0), with = "cbor::signature")]
        signature: Signature,
        /// The transaction message
        #[cbor(n(1), with = "cbor::message")]
        transaction: Message,
        /// The signing accounts in signature order (fee payer first)
        #[n(2)]
        signers: Vec<Account>,
    },
    /// Deposited funds from user deposit accounts have been consolidated
    /// into the minter's main account.
    #[n(7)]
    ConsolidatedDeposits {
        /// The deposit accounts from which funds were consolidated
        /// and the amount consolidated from each account.
        #[n(0)]
        deposits: Vec<(Account, Lamport)>,
    },
    /// A withdrawal transaction was signed and is ready to be sent to the network.
    #[n(8)]
    SentWithdrawalTransaction {
        /// The withdrawal request included in this transaction.
        #[n(0)]
        request: WithdrawSolRequest,
        /// The transaction signature.
        #[cbor(n(1), with = "cbor::signature")]
        signature: Signature,
        /// The transaction message.
        #[cbor(n(2), with = "cbor::message")]
        transaction: Message,
    },
}

/// Payload of the `AcceptedWithdrawSolRequest` event.
#[derive(Clone, Eq, PartialEq, Debug, Decode, Encode)]
pub struct WithdrawSolRequest {
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
    pub withdrawal_amount: Lamport,
    /// The fee retained by the minter (in lamports).
    #[n(4)]
    pub withdrawal_fee: Lamport,
}

#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd, Debug, Decode, Encode)]
pub struct DepositId {
    #[cbor(n(0), with = "cbor::signature")]
    pub signature: Signature,
    #[n(1)]
    pub account: Account,
}

impl Storable for Event {
    fn to_bytes(&self) -> Cow<'_, [u8]> {
        let mut buf = vec![];
        minicbor::encode(self, &mut buf).expect("event encoding should always succeed");
        Cow::Owned(buf)
    }

    fn into_bytes(self) -> Vec<u8> {
        self.to_bytes().into_owned()
    }

    fn from_bytes(bytes: Cow<[u8]>) -> Self {
        minicbor::decode(bytes.as_ref())
            .unwrap_or_else(|e| panic!("failed to decode event bytes {}: {e}", hex::encode(bytes)))
    }

    const BOUND: Bound = Bound::Unbounded;
}
