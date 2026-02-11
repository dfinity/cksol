use minicbor::{Decode, Encode};

/// The minter minted some ckSOL tokens.
#[derive(Clone, Eq, PartialEq, Debug, Decode, Encode)]
pub enum MintMemo<'a> {
    #[n(0)]
    /// The minter converted a deposit transaction to ckSOL.
    Convert {
        #[cbor(n(0), with = "minicbor::bytes")]
        /// The transaction signature of the accepted deposit.
        signature: &'a [u8],
    },
}
