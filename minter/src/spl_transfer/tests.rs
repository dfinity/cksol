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

#[test]
fn should_build_one_spl_withdrawal_with_idempotent_account_creation_and_one_signature() {
    use crate::{
        address::associated_token_address, spl_transfer::withdrawal::SplWithdrawalTransfer,
        state::SupportedSplToken,
    };
    use candid::Principal;
    use solana_hash::Hash;

    let minter = Address::from([1; 32]);
    let nonce_account = Address::from([5; 32]);
    let destination = Address::from([3; 32]);
    let token = SupportedSplToken {
        mint: Address::from([2; 32]),
        token_program: spl_token_interface::id().to_bytes().into(),
        decimals: 6,
        ledger_id: Principal::from_slice(&[1]),
        index_id: None,
        symbol: "ckTest".to_string(),
        minimum_deposit_amount: 1,
        deposit_fee: 0,
        deposits_paused: false,
        withdrawal_fee_token: 0,
    };
    let nonce_value = Hash::new_from_array([7; 32]);

    let message = build_spl_withdrawal_message(
        &minter,
        &nonce_account,
        nonce_value,
        &SplWithdrawalTransfer {
            token: &token,
            destination: &destination,
            amount: 123_456,
        },
    )
    .unwrap();

    // The minter is the fee payer and the only signer; the nonce replaces the blockhash.
    assert_eq!(message.header.num_required_signatures, 1);
    assert_eq!(message.account_keys[0], minter);
    assert_eq!(message.recent_blockhash, nonce_value);
    let program = |index: usize| {
        message.account_keys[usize::from(message.instructions[index].program_id_index)]
    };
    let accounts = |index: usize| -> Vec<Address> {
        message.instructions[index]
            .accounts
            .iter()
            .map(|account| message.account_keys[usize::from(*account)])
            .collect()
    };
    assert_eq!(message.instructions.len(), 3);

    // 1. The nonce is advanced first.
    assert_eq!(program(0), solana_sdk_ids::system_program::ID);
    assert_eq!(accounts(0)[0], nonce_account);

    // 2. The destination token account is created unless it exists, paid by the minter.
    let treasury = associated_token_address(&minter, &token.mint);
    let destination_token_account = associated_token_address(&destination, &token.mint);
    assert_eq!(
        program(1).to_bytes(),
        spl_associated_token_account_interface::program::id().to_bytes()
    );
    // The instruction data 1 selects `CreateIdempotent`.
    assert_eq!(message.instructions[1].data, [1]);
    assert_eq!(
        accounts(1)[..4],
        [minter, destination_token_account, destination, token.mint]
    );

    // 3. The net amount moves from the treasury to the destination token account.
    assert_eq!(program(2).to_bytes(), spl_token_interface::id().to_bytes());
    let mut data = vec![12];
    data.extend(123_456_u64.to_le_bytes());
    data.push(6);
    assert_eq!(message.instructions[2].data, data);
    assert_eq!(
        accounts(2),
        [treasury, token.mint, destination_token_account, minter]
    );
}
