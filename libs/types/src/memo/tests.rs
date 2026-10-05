use crate::{
    BurnMemo, DepositSolId,
    memo::{MAX_SERIALIZED_MEMO_BYTES, Memo as CkSolMinterMemo, MintMemo},
};
use icrc_ledger_types::icrc1::transfer::Memo as Icrc1Memo;
use proptest::{
    array::uniform,
    prelude::{Strategy, any},
    prop_assert, prop_oneof, proptest,
};

/// The `max_memo_length` that ICRC-1 ledgers are typically deployed with.
const TYPICAL_LEDGER_MAX_MEMO_BYTES: usize = 80;

/// The largest deposit id whose mint memo still fits into a typical ledger memo.
///
/// CBOR spends five bytes on an unsigned integer up to [`u32::MAX`] and nine on any
/// larger one, so the memo outgrows [`TYPICAL_LEDGER_MAX_MEMO_BYTES`] exactly there.
const LARGEST_DEPOSIT_ID_WITHIN_TYPICAL_LEDGER_MEMO: DepositSolId = u32::MAX as DepositSolId;

#[test]
fn should_reach_the_maximum_size_with_the_largest_sweep_memo() {
    assert_eq!(
        encoded_len(sweep_memo(DepositSolId::MAX)),
        MAX_SERIALIZED_MEMO_BYTES as usize
    );
}

#[test]
fn should_outgrow_a_typical_ledger_memo_one_deposit_id_past_the_largest_u32() {
    let largest = LARGEST_DEPOSIT_ID_WITHIN_TYPICAL_LEDGER_MEMO;

    let within = encoded_len(sweep_memo(largest));
    let beyond = encoded_len(sweep_memo(largest + 1));

    assert!(within <= TYPICAL_LEDGER_MAX_MEMO_BYTES);
    assert!(beyond > TYPICAL_LEDGER_MAX_MEMO_BYTES);
}

proptest! {
    #[test]
    fn should_never_exceed_maximum_size(memo in arb_memo()) {
        let encoded = Icrc1Memo::from(memo);

        prop_assert!(encoded.0.len() <= MAX_SERIALIZED_MEMO_BYTES as usize);
    }

    #[test]
    fn should_fit_into_a_typical_ledger_memo_up_to_the_largest_u32_deposit_id(
        memo in arb_sweep_mint_memo(0..=LARGEST_DEPOSIT_ID_WITHIN_TYPICAL_LEDGER_MEMO),
    ) {
        let encoded = Icrc1Memo::from(CkSolMinterMemo::Mint(memo));

        prop_assert!(encoded.0.len() <= TYPICAL_LEDGER_MAX_MEMO_BYTES);
    }

    #[test]
    fn should_outgrow_a_typical_ledger_memo_beyond_the_largest_u32_deposit_id(
        memo in arb_sweep_mint_memo(
            (LARGEST_DEPOSIT_ID_WITHIN_TYPICAL_LEDGER_MEMO + 1)..=DepositSolId::MAX,
        ),
    ) {
        let encoded = Icrc1Memo::from(CkSolMinterMemo::Mint(memo));

        prop_assert!(encoded.0.len() > TYPICAL_LEDGER_MAX_MEMO_BYTES);
    }
}

fn sweep_memo(deposit_id: DepositSolId) -> CkSolMinterMemo {
    CkSolMinterMemo::Mint(MintMemo::Sweep {
        signature: [0xAB; 64],
        deposit_id,
    })
}

fn encoded_len(memo: CkSolMinterMemo) -> usize {
    Icrc1Memo::from(memo).0.len()
}

fn arb_memo() -> impl Strategy<Value = CkSolMinterMemo> {
    prop_oneof![
        arb_mint_memo().prop_map(CkSolMinterMemo::Mint),
        arb_burn_memo().prop_map(CkSolMinterMemo::Burn)
    ]
}

fn arb_mint_memo() -> impl Strategy<Value = MintMemo> {
    arb_sweep_mint_memo(any::<DepositSolId>())
}

fn arb_sweep_mint_memo(
    deposit_ids: impl Strategy<Value = DepositSolId>,
) -> impl Strategy<Value = MintMemo> {
    (arb_signature(), deposit_ids).prop_map(|(signature, deposit_id)| MintMemo::Sweep {
        signature: signature.into(),
        deposit_id,
    })
}

fn arb_signature() -> impl Strategy<Value = solana_signature::Signature> {
    uniform::<_, 64>(any::<u8>()).prop_map(solana_signature::Signature::from)
}

fn arb_burn_memo() -> impl Strategy<Value = BurnMemo> {
    any::<[u8; 32]>().prop_map(|to_address| BurnMemo::Convert { to_address })
}
