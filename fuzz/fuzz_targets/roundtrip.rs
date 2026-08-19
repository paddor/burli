#![no_main]

use std::io::Read;
use std::io::Write;

use libfuzzer_sys::fuzz_target;

fuzz_target!(|input: &[u8]| {
    for quality in 0..=5 {
        let encoded = burli::compress(input, quality).unwrap();
        burli::validate(&encoded).unwrap();
        let decoded = burli::decompress(&encoded).unwrap();
        assert_eq!(decoded, input);

        let mut stream_encoder = burli::StreamEncoder::new(Vec::new(), quality).unwrap();
        for chunk in input.chunks(17) {
            stream_encoder.write_all(chunk).unwrap();
        }
        let stream_encoded = stream_encoder.finish().unwrap();
        burli::validate(&stream_encoded).unwrap();
        assert_eq!(burli::decompress(&stream_encoded).unwrap(), input);

        let mut decoder = rust_brotli::Decompressor::new(encoded.as_slice(), 4096);
        let mut rust_brotli_decoded = Vec::new();
        decoder.read_to_end(&mut rust_brotli_decoded).unwrap();
        assert_eq!(rust_brotli_decoded, input);
    }
});
