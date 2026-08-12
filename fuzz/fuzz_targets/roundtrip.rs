#![no_main]

use std::io::Read;

use libfuzzer_sys::fuzz_target;

fuzz_target!(|input: &[u8]| {
    for quality in 0..=5 {
        let encoded = burli::compress(input, quality).unwrap();
        let decoded = burli::decompress(&encoded).unwrap();
        assert_eq!(decoded, input);

        let mut decoder = rust_brotli::Decompressor::new(encoded.as_slice(), 4096);
        let mut rust_brotli_decoded = Vec::new();
        decoder.read_to_end(&mut rust_brotli_decoded).unwrap();
        assert_eq!(rust_brotli_decoded, input);
    }
});
