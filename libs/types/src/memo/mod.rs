use crate::DepositSolId;
use derive_more::From;
use minicbor::{Decode, Encode, Encoder};
use solana_address::Address;
use solana_signature::SIGNATURE_BYTES;

#[cfg(test)]
mod tests;

/// Maximum size in bytes of a [`Memo`] when serialized into an [ICRC-1 memo].
///
/// The largest memo is a [`MintMemo::Sweep`], which encodes to 81 bytes:
///
/// | Bytes | Content |
/// | --- | --- |
/// | 2 | the [`Memo`] enum, as an array holding the index of [`Memo::Mint`] |
/// | 1 | the array holding the single field of [`Memo::Mint`] |
/// | 2 | the [`MintMemo`] enum, as an array holding the index of [`MintMemo::Sweep`] |
/// | 1 | the array holding the two fields of [`MintMemo::Sweep`] |
/// | 2 | the byte string header of the signature, whose length needs a byte of its own |
/// | 64 | the sweep signature |
/// | 9 | the deposit id, as an unsigned integer |
///
/// Only the deposit id varies, since CBOR encodes an integer in as few bytes as
/// possible: it takes nine bytes above [`u32::MAX`] and at most five below, which
/// keeps the memo within 80 bytes for every deposit id reachable in practice.
///
/// A ledger whose `max_memo_length` is below this value rejects the mints whose memo
/// exceeds its limit, and one below the size of the smallest memo, such as the ICRC-1
/// default of 32 bytes, rejects all of them.
///
/// # Example
///
/// ```rust
/// use cksol_types::{Memo, MintMemo, MAX_SERIALIZED_MEMO_BYTES};
/// use icrc_ledger_types::icrc1::transfer::Memo as Icrc1Memo;
/// use solana_signature::Signature;
/// use std::str::FromStr;
///
/// let signature = Signature::from_str("5pf5fC9WRhdvE5y6eUkxons4btM3Tfi7koj4W1Q2kLztP8oZoLVn516XuuvG7cY61wLoyVAoakm1wz1z8V67rvh").unwrap();
///
/// let memo = Memo::Mint(MintMemo::Convert {
///     signature: signature.into()
/// });
///
/// assert!(Icrc1Memo::from(memo).0.len() <= MAX_SERIALIZED_MEMO_BYTES as usize)
/// ```
///
/// [ICRC-1 memo]: icrc_ledger_types::icrc1::transfer::Memo
pub const MAX_SERIALIZED_MEMO_BYTES: u16 = 81;

/// A ckSOL minter ledger memo.
#[derive(Clone, Eq, PartialEq, Debug, Decode, Encode, From)]
pub enum Memo {
    /// The minter minted some ckSOL tokens.
    #[n(0)]
    Mint(#[n(0)] MintMemo),
    /// The minter burned some ckSOL tokens.
    #[n(1)]
    Burn(#[n(0)] BurnMemo),
}

/// The minter minted some ckSOL tokens.
#[derive(Clone, Eq, PartialEq, Debug, Decode, Encode)]
pub enum MintMemo {
    /// The minter converted a deposit transaction to ckSOL.
    #[n(0)]
    Convert {
        /// The transaction signature of the accepted deposit.
        #[cbor(n(0), with = "minicbor::bytes")]
        signature: [u8; 64],
    },
    /// The minter converted a deposit swept to its main account to ckSOL.
    #[n(1)]
    Sweep {
        /// The transaction signature of the sweep that moved the deposit.
        #[cbor(n(0), with = "minicbor::bytes")]
        signature: [u8; 64],
        /// The identifier of the swept deposit in the minter.
        #[n(1)]
        deposit_id: DepositSolId,
    },
}

/// The minter burned some ckSOL tokens.
#[derive(Clone, Eq, PartialEq, Debug, Decode, Encode)]
pub enum BurnMemo {
    /// The minter burned ckSOL to initiate the withdrawal.
    #[n(0)]
    Convert {
        /// The solana withdrawal address.
        #[cbor(n(0), with = "minicbor::bytes")]
        to_address: [u8; 32],
    },
}

impl MintMemo {
    /// Create a [`MintMemo::Convert`] memo instance from a [`Signature`].
    ///
    /// [`Signature`]: solana_signature::Signature
    pub fn convert(signature: impl Into<solana_signature::Signature>) -> Self {
        Self::Convert {
            signature: <[u8; SIGNATURE_BYTES]>::from(signature.into()),
        }
    }

    /// Create a [`MintMemo::Sweep`] memo instance from the [`Signature`] of the sweep
    /// and the identifier of the swept deposit.
    ///
    /// [`Signature`]: solana_signature::Signature
    pub fn sweep(
        signature: impl Into<solana_signature::Signature>,
        deposit_id: DepositSolId,
    ) -> Self {
        Self::Sweep {
            signature: <[u8; SIGNATURE_BYTES]>::from(signature.into()),
            deposit_id,
        }
    }
}

impl BurnMemo {
    /// Create a [`BurnMemo::Convert`] memo instance from an [`Address`].
    ///
    /// [`Address`]: to_address::Address
    pub fn convert(to_address: Address) -> Self {
        Self::Convert {
            to_address: to_address.to_bytes(),
        }
    }
}

impl From<Memo> for icrc_ledger_types::icrc1::transfer::Memo {
    fn from(memo: Memo) -> icrc_ledger_types::icrc1::transfer::Memo {
        let mut encoder = Encoder::new(Vec::new());
        encoder.encode(&memo).expect("minicbor encoding failed");
        icrc_ledger_types::icrc1::transfer::Memo::from(encoder.into_writer())
    }
}
