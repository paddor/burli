#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|input: &[u8]| {
    let _ = burli::decompress_with_limit(input, 1 << 20);
});
