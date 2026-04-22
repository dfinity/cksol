use crate::{
    state::event::VersionedMessage,
    test_fixtures::arb::{arb_message, arb_signature},
    utils::cbor,
};
use proptest::{prop_assert_eq, proptest};

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

mod signature_option_tests {
    use super::*;

    #[test]
    fn none_roundtrips() {
        let encoded = encode_opt(None);
        assert_eq!(decode_opt(&encoded), None);
    }

    proptest! {
        #[test]
        fn some_minicbor_roundtrip(signature in arb_signature()) {
            let encoded = encode_opt(Some(signature));
            prop_assert_eq!(decode_opt(&encoded), Some(signature));
        }
    }

    fn encode_opt(v: Option<solana_signature::Signature>) -> Vec<u8> {
        let mut buf = Vec::new();
        let mut encoder = minicbor::Encoder::new(&mut buf);
        cbor::signature::option::encode(&v, &mut encoder, &mut ()).unwrap();
        buf
    }

    fn decode_opt(bytes: &[u8]) -> Option<solana_signature::Signature> {
        let mut decoder = minicbor::Decoder::new(bytes);
        cbor::signature::option::decode(&mut decoder, &mut ()).unwrap()
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
