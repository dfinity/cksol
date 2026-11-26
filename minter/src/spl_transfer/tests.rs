use super::*;

#[test]
fn should_encode_the_classic_transfer_and_account_permissions() {
    let source = Address::from([1; 32]);
    let mint = Address::from([2; 32]);
    let destination = Address::from([3; 32]);
    let authority = Address::from([4; 32]);
    for (amount, decimals) in [(1, 0), (123_456_789, 6), (u64::MAX, u8::MAX)] {
        let instruction = transfer_checked_instruction(
            &source,
            &mint,
            &destination,
            &authority,
            amount,
            decimals,
        );
        assert_eq!(
            Address::from(instruction.program_id.to_bytes()),
            "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA"
                .parse::<Address>()
                .unwrap(),
        );
        // SPL Token's TransferChecked discriminant, little-endian amount and decimals.
        let mut data = vec![12];
        data.extend(amount.to_le_bytes());
        data.push(decimals);
        assert_eq!(instruction.data, data);
        assert_eq!(instruction.accounts.len(), 4);
        let expected = [
            (source, false, true),
            (mint, false, false),
            (destination, false, true),
            (authority, true, false),
        ];
        for (actual, (address, signer, writable)) in instruction.accounts.iter().zip(expected) {
            assert_eq!(actual.pubkey.to_bytes(), address.to_bytes());
            assert_eq!(actual.is_signer, signer);
            assert_eq!(actual.is_writable, writable);
        }
    }
}
