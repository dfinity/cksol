use crate::memo::{MAX_SERIALIZED_MEMO_BYTES, Memo as CkSolMinterMemo, MintMemo};
use icrc_ledger_types::icrc1::transfer::Memo as Icrc1Memo;
use proptest::prelude::*;

proptest! {
    #[test]
    fn should_never_exceed_maximum_size(memo in arb_memo()) {
        let encoded = Icrc1Memo::from(memo);

        prop_assert!(encoded.0.len() <= MAX_SERIALIZED_MEMO_BYTES as usize);
    }
}

fn arb_memo() -> impl Strategy<Value = CkSolMinterMemo> {
    arb_mint_memo().prop_map(CkSolMinterMemo::Mint)
}

fn arb_mint_memo() -> impl Strategy<Value = MintMemo> {
    arb_signature().prop_map(|signature| MintMemo::Convert {
        signature: signature.into(),
    })
}

fn arb_signature() -> impl Strategy<Value = solana_signature::Signature> {
    prop::array::uniform::<_, 64>(any::<u8>()).prop_map(solana_signature::Signature::from)
}
