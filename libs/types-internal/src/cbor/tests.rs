use proptest::prelude::*;

fn encode_u128(value: u128) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut encoder = minicbor::Encoder::new(&mut buf);
    super::u128::encode(&value, &mut encoder, &mut ()).unwrap();
    buf
}

fn decode_u128(bytes: &[u8]) -> u128 {
    let mut decoder = minicbor::Decoder::new(bytes);
    super::u128::decode(&mut decoder, &mut ()).unwrap()
}

fn encode_option_u128(value: Option<u128>) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut encoder = minicbor::Encoder::new(&mut buf);
    super::u128::option::encode(&value, &mut encoder, &mut ()).unwrap();
    buf
}

fn decode_option_u128(bytes: &[u8]) -> Option<u128> {
    let mut decoder = minicbor::Decoder::new(bytes);
    super::u128::option::decode(&mut decoder, &mut ()).unwrap()
}

proptest! {
    #[test]
    fn u128_roundtrip(value: u128) {
        let encoded = encode_u128(value);
        let decoded = decode_u128(&encoded);
        prop_assert_eq!(value, decoded);
    }

    #[test]
    fn option_u128_roundtrip(value: Option<u128>) {
        let encoded = encode_option_u128(value);
        let decoded = decode_option_u128(&encoded);
        prop_assert_eq!(value, decoded);
    }
}
