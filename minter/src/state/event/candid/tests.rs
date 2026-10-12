use crate::{
    constants::RENT_EXEMPTION_THRESHOLD,
    state::event::{Event, EventType, candid::InvalidCandidEvent},
    test_fixtures::arb::{arb_account, arb_address, arb_event, arb_signature},
};
use assert_matches::assert_matches;
use cksol_types_internal::event::{
    Event as CandidEvent, EventType as CandidEventType, Signer as CandidSigner,
    TransactionPurpose as CandidTransactionPurpose, VersionedTransactionMessage,
};
use proptest::{prelude::Strategy, prop_assert, prop_assert_eq, proptest};

proptest! {
    #[test]
    fn candid_roundtrip(event in arb_event()) {
        let expected = Event { timestamp: event.timestamp, payload: event.payload.clone() };
        let candid_event = CandidEvent::from(event);
        let roundtripped = Event::try_from(candid_event).unwrap();
        prop_assert_eq!(roundtripped, expected);
    }

    #[test]
    fn should_reject_a_message_that_is_not_bincode(signature in arb_signature()) {
        let payload = CandidEventType::SubmittedTransaction {
            signature: signature.into(),
            transaction: VersionedTransactionMessage::Legacy(vec![0xff; 3]),
            signers: vec![CandidSigner::Minter],
            purpose: CandidTransactionPurpose::Withdrawal { burn_indices: vec![1] },
        };
        prop_assert!(matches!(
            EventType::try_from(payload).unwrap_err(),
            InvalidCandidEvent::InvalidMessage(_)
        ));
    }

    #[test]
    fn should_reject_a_deposit_balance_below_the_rent_exemption_threshold(
        account in arb_account(),
        address in arb_address(),
        balance in 0..RENT_EXEMPTION_THRESHOLD,
    ) {
        let payload = CandidEventType::QueuedDeposit {
            deposit_id: 0,
            account,
            address: address.into(),
            balance,
        };
        prop_assert_eq!(
            EventType::try_from(payload).unwrap_err(),
            InvalidCandidEvent::DepositBalanceBelowRentExemption(balance)
        );
    }

    #[test]
    fn should_reject_a_chain_code_that_is_not_32_bytes(
        len in (0_usize..64).prop_filter("32 bytes is a valid chain code", |len| *len != 32)
    ) {
        let payload = CandidEventType::MinterPublicKeyFetched {
            public_key: valid_public_key(),
            chain_code: vec![0; len],
        };
        prop_assert_eq!(
            EventType::try_from(payload).unwrap_err(),
            InvalidCandidEvent::InvalidChainCode(len)
        );
    }
}

#[test]
fn should_reject_a_public_key_that_is_not_32_bytes() {
    let payload = CandidEventType::MinterPublicKeyFetched {
        public_key: vec![0xff; 31],
        chain_code: vec![0; 32],
    };
    assert_matches!(
        EventType::try_from(payload).unwrap_err(),
        InvalidCandidEvent::InvalidPublicKey(_)
    );
}

fn valid_public_key() -> Vec<u8> {
    ic_ed25519::PrivateKey::generate()
        .public_key()
        .serialize_raw()
        .to_vec()
}
