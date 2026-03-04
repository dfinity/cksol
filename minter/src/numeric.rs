use phantom_newtype::Id;

pub enum MintIndexTag {}
pub type LedgerMintIndex = Id<MintIndexTag, u64>;
