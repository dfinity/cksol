use crate::{
    state::event::cbor,
    test_fixtures::arb::{arb_signature, arb_message},
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
