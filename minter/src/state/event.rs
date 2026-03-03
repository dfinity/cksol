use crate::numeric::LedgerMintIndex;
use cksol_types_internal::{InitArgs, UpgradeArgs};
use ic_stable_structures::Storable;
use ic_stable_structures::storable::Bound;
use icrc_ledger_types::icrc1::account::Account;
use minicbor::{Decode, Encode};
use sol_rpc_types::Lamport;
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
