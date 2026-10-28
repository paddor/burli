//! Compression must fit small thread stacks: .NET 8 threads on macOS and
//! Ruby fibers get 512 KiB. Hash tables on the stack once took up to 608 KiB
//! with LTO.

use std::thread;

/// Debug builds need about 124 KiB here, release builds about 90 KiB.
const STACK: usize = 192 << 10;

fn xorshift(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

/// Text, then noise, then text: 80 KiB that reach every hash table size.
fn mixed() -> Vec<u8> {
    let words: [&[u8]; 6] = [
        b"brotli ",
        b"window ",
        b"literal ",
        b"context ",
        b"copy ",
        b"the\n",
    ];
    let mut state = 0x9e37_79b9_7f4a_7c15;
    let mut text = Vec::new();
    while text.len() < 30_000 {
        text.extend_from_slice(words[(xorshift(&mut state) % 6) as usize]);
    }
    let noise: Vec<u8> = (0..20_000).map(|_| xorshift(&mut state) as u8).collect();
    [text.as_slice(), &noise, &text].concat()
}

#[test]
fn every_quality_and_window_fits_a_small_stack() {
    let input = mixed();
    thread::Builder::new()
        .stack_size(STACK)
        .spawn(move || {
            for quality in 0..=5 {
                for window in [10, 16, 22, 24] {
                    let options = burli::Options::default()
                        .with_quality(quality)
                        .and_then(|options| options.with_window_bits(window))
                        .unwrap();
                    let encoded = burli::compress_with_options(&input, &options).unwrap();
                    assert_eq!(
                        burli::decompress(&encoded).unwrap(),
                        input,
                        "q{quality} w{window}"
                    );
                }
            }
        })
        .unwrap()
        .join()
        .unwrap();
}
