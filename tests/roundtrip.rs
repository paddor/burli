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

proptest! {
    #![proptest_config(ProptestConfig::with_cases(16))]

    #[test]
    #[cfg(feature = "std")]
    fn streaming_encoder_round_trip_arbitrary_bytes(
        input in prop::collection::vec(any::<u8>(), 0..512),
        chunk in 1usize..64,
    ) {
        for quality in 0..=5 {
            let mut encoder = burli::StreamEncoder::new(Vec::new(), quality).unwrap();
            for part in input.chunks(chunk) {
                use std::io::Write as _;
                encoder.write_all(part).unwrap();
            }
            let encoded = encoder.finish().unwrap();

            let decoded = burli::decompress(&encoded).unwrap();
            prop_assert_eq!(&decoded, &input);
        }
    }

    #[test]
    fn burli_decodes_rust_brotli_arbitrary_small_streams(
        input in prop::collection::vec(any::<u8>(), 0..256),
        quality in 0u32..=11,
    ) {
        let mut encoder = rust_brotli::CompressorReader::new(input.as_slice(), 4096, quality, 22);
        let mut encoded = Vec::new();
        encoder.read_to_end(&mut encoded).unwrap();

        let decoded = burli::decompress(&encoded).unwrap();
        prop_assert_eq!(&decoded, &input);
    }
}

/// Text blocks around a low-compressibility block make the encoder switch
/// collectors between meta-blocks. The decoder keeps its distance ring across
/// meta-blocks, so the encoder must too.
fn mixed_collector_input() -> Vec<u8> {
    const BLOCK: usize = 1 << 16;
    let mut state = 0x9e37_79b9_7f4a_7c15_u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let pattern: Vec<u8> = (0..100).map(|_| b'a' + (next() % 26) as u8).collect();
    let mut input: Vec<u8> = pattern.iter().copied().cycle().take(BLOCK).collect();
    let binary_start = input.len();
    input.extend((0..BLOCK).map(|_| next() as u8));
    for copy in 0..2 {
        let dst = binary_start + 4096 + copy * 7919;
        input.copy_within(dst - 3072..dst - 3072 + 1024, dst);
    }
    input.extend(pattern.iter().copied().cycle().take(BLOCK));
    // A short final block takes the q0 direct path. Its first copy has the
    // initial last distance, which the ring no longer holds.
    input.extend_from_slice(b"wxyzwxyzwxyz");
    input.extend((0..288).map(|_| next() as u8));
    input
}

fn decode_failures(encoded: &[u8], input: &[u8], label: &str) -> Vec<String> {
    let mut failures = Vec::new();
    if burli::decompress(encoded).ok().as_deref() != Some(input) {
        failures.push(format!("burli: {label}"));
    }
    let mut decoder = rust_brotli::Decompressor::new(encoded, 4096);
    let mut decoded = Vec::new();
    if decoder.read_to_end(&mut decoded).is_err() || decoded != input {
        failures.push(format!("rust-brotli: {label}"));
    }
    failures
}

#[test]
fn distance_ring_survives_collector_switches() {
    let input = mixed_collector_input();
    let mut failures = Vec::new();
    for quality in 0..=5 {
        let options = burli::Options::default()
            .with_quality(quality)
            .unwrap()
            .with_block_bits(Some(16))
            .unwrap();
        let encoded = burli::compress_with_options(&input, &options).unwrap();
        failures.extend(decode_failures(
            &encoded,
            &input,
            &format!("one-shot q{quality}"),
        ));
    }
    assert!(failures.is_empty(), "{failures:?}");
}

#[test]
#[cfg(feature = "std")]
fn streaming_distance_ring_survives_collector_switches() {
    use std::io::Write as _;

    let input = mixed_collector_input();
    let mut failures = Vec::new();
    for quality in 0..=5 {
        let mut encoder = burli::StreamEncoder::new(Vec::new(), quality).unwrap();
        encoder.write_all(&input).unwrap();
        let encoded = encoder.finish().unwrap();
        failures.extend(decode_failures(
            &encoded,
            &input,
            &format!("streaming q{quality}"),
        ));
    }
    assert!(failures.is_empty(), "{failures:?}");
}
