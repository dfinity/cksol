use super::*;
use solana_address::Address;

#[test]
fn system_transfer_instruction_encoding() {
    let from = Address::from([1u8; 32]);
    let to = Address::from([2u8; 32]);
    let lamports: Lamport = 1_000_000_000;

    let ix = system_transfer_instruction(&from, &to, lamports);

    assert_eq!(ix.program_id, SYSTEM_PROGRAM_ID);
    assert_eq!(ix.accounts.len(), 2);
    assert_eq!(ix.accounts[0].pubkey, from);
    assert!(ix.accounts[0].is_signer);
    assert!(ix.accounts[0].is_writable);
    assert_eq!(ix.accounts[1].pubkey, to);
    assert!(!ix.accounts[1].is_signer);
    assert!(ix.accounts[1].is_writable);

    // Bincode: variant index 2 (u32 LE) + lamports (u64 LE)
    assert_eq!(ix.data.len(), 12);
    assert_eq!(&ix.data[..4], &2u32.to_le_bytes());
    assert_eq!(&ix.data[4..], &lamports.to_le_bytes());
}

#[test]
fn system_program_id_is_all_zeros() {
    assert_eq!(SYSTEM_PROGRAM_ID, Address::from([0u8; 32]));
    assert_eq!(
        SYSTEM_PROGRAM_ID.to_string(),
        "11111111111111111111111111111111"
    );
}
