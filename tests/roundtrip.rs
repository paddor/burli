use std::io::Read;

use proptest::prelude::*;

proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]

    #[test]
    fn q0_to_q5_round_trip_arbitrary_bytes(input in prop::collection::vec(any::<u8>(), 0..1024)) {
        for quality in 0..=5 {
            let encoded = burli::compress(&input, quality).unwrap();
            let decoded = burli::decompress(&encoded).unwrap();
            prop_assert_eq!(&decoded, &input);
        }
    }

    #[test]
    fn q0_to_q5_outputs_decode_with_rust_brotli(input in prop::collection::vec(any::<u8>(), 0..128)) {
        for quality in 0..=5 {
            let encoded = burli::compress(&input, quality).unwrap();
            let mut decoder = rust_brotli::Decompressor::new(encoded.as_slice(), 4096);
            let mut decoded = Vec::new();

            decoder.read_to_end(&mut decoded).unwrap();
            prop_assert_eq!(&decoded, &input);
        }
    }
}
