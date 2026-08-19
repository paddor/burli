#![no_main]

use std::io::Read;

use libfuzzer_sys::fuzz_target;

fuzz_target!(|input: &[u8]| {
    let one_shot = burli::decompress_with_limit(input, 1 << 20);
    let validation = burli::validate(input);
    let mut decoder = burli::StreamDecoder::with_limit(input, 1 << 20);
    let mut streamed = Vec::new();
    let stream = decoder.read_to_end(&mut streamed);

    if let Ok(decoded) = one_shot {
        assert!(validation.is_ok());
        assert!(stream.is_ok());
        assert_eq!(streamed, decoded);
    }
});
