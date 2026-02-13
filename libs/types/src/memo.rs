use derive_more::From;
use minicbor::{Decode, Encode};
use solana_signature::SIGNATURE_BYTES;

/// A ckSOL minter ledger memo.
#[derive(Clone, Eq, PartialEq, Debug, Decode, Encode, From)]
pub enum Memo {
    /// The minter minted some ckSOL tokens.
    #[n(0)]
    Mint(#[n(0)] MintMemo),
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
