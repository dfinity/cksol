use crate::{
    constants::RENT_EXEMPTION_THRESHOLD,
    state::{
        DepositBalance,
        event::{VersionedMessage, cbor},
    },
    test_fixtures::arb::{arb_deposit_balance, arb_message, arb_signature},
};
use proptest::{prop_assert_eq, proptest};

mod deposit_balance_tests {
    use super::*;

    proptest! {
        #[test]
        fn deposit_balance_minicbor_roundtrip(balance in arb_deposit_balance()) {
            let encoded = encode_deposit_balance(&balance);
            let decoded = decode_deposit_balance(&encoded).unwrap();
            prop_assert_eq!(balance, decoded);
        }
    }

    #[test]
    fn should_reject_a_balance_below_the_rent_exemption_threshold() {
        for balance in [0, RENT_EXEMPTION_THRESHOLD - 1] {
            let mut encoded = Vec::new();
            minicbor::Encoder::new(&mut encoded).u64(balance).unwrap();

            let decoded = decode_deposit_balance(&encoded);

            assert!(
                decoded
                    .unwrap_err()
                    .to_string()
                    .contains("below the rent exemption threshold"),
                "balance {balance}"
            );
        }
    }

    fn encode_deposit_balance(balance: &DepositBalance) -> Vec<u8> {
        let mut buf = Vec::new();
        let mut encoder = minicbor::Encoder::new(&mut buf);
        cbor::deposit_balance::encode(balance, &mut encoder, &mut ()).unwrap();
        buf
    }

    fn decode_deposit_balance(bytes: &[u8]) -> Result<DepositBalance, minicbor::decode::Error> {
        let mut decoder = minicbor::Decoder::new(bytes);
        cbor::deposit_balance::decode(&mut decoder, &mut ())
    }
}

mod signature_tests {
    use super::*;

    proptest! {
        #[test]
        fn signature_minicbor_roundtrip(signature in arb_signature()) {
            let encoded = encode_signature(&signature);
            let decoded = decode_signature(&encoded);
            prop_assert_eq!(signature, decoded);
        }
    }

    fn encode_signature(signature: &solana_signature::Signature) -> Vec<u8> {
        let mut buf = Vec::new();
        let mut encoder = minicbor::Encoder::new(&mut buf);
        cbor::signature::encode(signature, &mut encoder, &mut ()).unwrap();
        buf
    }

    fn decode_signature(bytes: &[u8]) -> solana_signature::Signature {
        let mut decoder = minicbor::Decoder::new(bytes);
        cbor::signature::decode(&mut decoder, &mut ()).unwrap()
    }
}

mod message_tests {
    use super::*;

    proptest! {
        #[test]
        fn message_minicbor_roundtrip(message in arb_message()) {
            let encoded = encode_message(&message);
            let decoded = decode_message(&encoded);
            prop_assert_eq!(message, decoded);
        }
    }

    fn encode_message(message: &solana_message::Message) -> Vec<u8> {
        let mut buf = Vec::new();
        let mut encoder = minicbor::Encoder::new(&mut buf);
        cbor::message::encode(message, &mut encoder, &mut ()).unwrap();
        buf
    }

    fn decode_message(bytes: &[u8]) -> solana_message::Message {
        let mut decoder = minicbor::Decoder::new(bytes);
        cbor::message::decode(&mut decoder, &mut ()).unwrap()
    }
}

mod versioned_message_tests {
    use super::*;

    proptest! {
        #[test]
        fn versioned_message_minicbor_roundtrip(message in arb_message()) {
            let versioned = VersionedMessage::Legacy(message);
            let encoded = encode_versioned_message(&versioned);
            let decoded = decode_versioned_message(&encoded);
            prop_assert_eq!(versioned, decoded);
        }
    }

    fn encode_versioned_message(message: &VersionedMessage) -> Vec<u8> {
        let mut buf = Vec::new();
        minicbor::encode(message, &mut buf).unwrap();
        buf
    }

    fn decode_versioned_message(bytes: &[u8]) -> VersionedMessage {
        minicbor::decode(bytes).unwrap()
    }
}
