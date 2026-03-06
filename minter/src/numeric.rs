use phantom_newtype::Id;

pub enum MintIndexTag {}
pub type LedgerMintIndex = Id<MintIndexTag, u64>;

pub enum BurnIndexTag {}
pub type LedgerBurnIndex = Id<BurnIndexTag, u64>;
