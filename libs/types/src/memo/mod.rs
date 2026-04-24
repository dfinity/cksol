use derive_more::From;
use minicbor::{Decode, Encode, Encoder};
use solana_address::Address;
use solana_signature::SIGNATURE_BYTES;

#[cfg(test)]
mod tests;

/// Maximum size in bytes of a [`Memo`] when serialized into an [ICRC-1 memo].
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
pub const MAX_SERIALIZED_MEMO_BYTES: u16 = 80;

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
