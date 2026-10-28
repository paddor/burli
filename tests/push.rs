//! A `PushDecoder` must decode like the one-shot decoder in any split of its
//! input and output, never take input past the stream end, and fail alike
//! in any split.

#![cfg(feature = "std")]

use std::io::Write;

use burli::{BurliError, DecodeStatus, PushDecoder};

/// How a push decode ended.
#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    /// The stream ended after `consumed` input bytes.
    Finished {
        consumed: usize,
    },
    /// The decoder took all input and needs more.
    Truncated,
    Failed(BurliError),
}

/// Pseudo-random piece sizes from 1 to `max`.
struct Pieces {
    state: u64,
    max: usize,
}

impl Pieces {
    const fn new(seed: u64, max: usize) -> Self {
        Self {
            state: seed | 1,
            max,
        }
    }

    fn next(&mut self) -> usize {
        if self.max == usize::MAX {
            return usize::MAX;
        }
        self.state ^= self.state << 13;
        self.state ^= self.state >> 7;
        self.state ^= self.state << 17;
        (self.state % self.max as u64) as usize + 1
    }
}

/// Decodes `encoded` with input and output pieces of at most `max_input`
/// and `max_output` bytes, and checks each call against the contract.
fn push_decode(
    mut decoder: PushDecoder,
    encoded: &[u8],
    seed: u64,
    max_input: usize,
    max_output: usize,
) -> (Vec<u8>, Outcome) {
    let mut inputs = Pieces::new(seed, max_input);
    let mut outputs = Pieces::new(seed.rotate_left(17), max_output);
    let mut output = Vec::new();
    let mut scratch = vec![0; max_output.min(1 << 18)];
    let mut pos = 0;
    let mut error_due = false;
    loop {
        let end = pos + inputs.next().min(encoded.len() - pos);
        let room = outputs.next().min(scratch.len());
        let result = decoder.decompress(&encoded[pos..end], &mut scratch[..room]);
        let progress = match result {
            Ok(progress) => progress,
            Err(error) => {
                assert!(
                    !error_due || decoder.decompress(&[], &mut [0; 8]) == Err(error.clone()),
                    "a failed decoder must fail again alike"
                );
                return (output, Outcome::Failed(error));
            }
        };
        assert!(!error_due, "the call after a short NeedsOutput must fail");
        assert!(progress.consumed <= end - pos, "{progress:?}");
        assert!(progress.written <= room, "{progress:?}");
        output.extend_from_slice(&scratch[..progress.written]);
        pos += progress.consumed;
        match progress.status {
            DecodeStatus::Finished => {
                assert!(decoder.is_finished());
                let again = decoder.decompress(&encoded[pos..], &mut [0; 8]).unwrap();
                assert_eq!((again.consumed, again.written), (0, 0));
                return (output, Outcome::Finished { consumed: pos });
            }
            DecodeStatus::NeedsInput => {
                assert_eq!(pos, end, "NeedsInput must take all input");
                if pos == encoded.len() {
                    return (output, Outcome::Truncated);
                }
            }
            // Only a decoder with an error to return writes less than the
            // room it was given.
            DecodeStatus::NeedsOutput => error_due = progress.written < room,
        }
    }
}

/// Split schedules as seed, largest input piece, and largest output piece.
const SCHEDULES: [(u64, usize, usize); 5] = [
    (1, usize::MAX, usize::MAX),
    (2, 17, usize::MAX),
    (3, usize::MAX, 29),
    (4, 3000, 5000),
    (5, 1, 7),
];

fn text(len: usize) -> Vec<u8> {
    let words = [
        "brotli ",
        "window ",
        "meta-block ",
        "literal ",
        "context ",
        "copy ",
        "the ",
        "of ",
        "distance\n",
        "decoder, ",
        "Huffman ",
        "code ",
    ];
    let mut state = 0x9e37_79b9_u32;
    let mut text = Vec::with_capacity(len + 16);
    while text.len() < len {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        text.extend_from_slice(words[(state >> 28) as usize % words.len()].as_bytes());
    }
    text.truncate(len);
    text
}

fn noise(len: usize, seed: u32) -> Vec<u8> {
    let mut state = seed;
    (0..len)
        .map(|_| {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (state >> 24) as u8
        })
        .collect()
}

fn rust_brotli(input: &[u8], quality: i32, lgwin: i32, lgblock: i32) -> Vec<u8> {
    let params = rust_brotli::enc::BrotliEncoderParams {
        quality,
        lgwin,
        lgblock,
        ..Default::default()
    };
    let mut encoded = Vec::new();
    {
        let mut writer = rust_brotli::CompressorWriter::with_params(&mut encoded, 4096, &params);
        writer.write_all(input).unwrap();
    }
    encoded
}

/// Streams as label, payload, and encoded bytes.
fn streams() -> Vec<(String, Vec<u8>, Vec<u8>)> {
    let mut mixed = text(40_000);
    mixed.extend(noise(20_000, 7));
    mixed.extend(text(30_000));
    let payloads = [
        ("empty", Vec::new()),
        ("byte", b"x".to_vec()),
        ("text", text(120_000)),
        ("noise", noise(50_000, 3)),
        ("mixed", mixed),
        ("zeros", vec![0; 200_000]),
    ];
    let mut streams = Vec::new();
    for (name, payload) in payloads {
        for quality in 0..=5 {
            let encoded = burli::compress(&payload, quality).unwrap();
            streams.push((format!("{name} burli q{quality}"), payload.clone(), encoded));
        }
        for (quality, lgwin, lgblock) in [(1, 22, 0), (5, 16, 24), (9, 10, 16)] {
            let encoded = rust_brotli(&payload, quality, lgwin, lgblock);
            streams.push((
                format!("{name} rust-brotli q{quality} lgwin {lgwin} lgblock {lgblock}"),
                payload.clone(),
                encoded,
            ));
        }
        let short = &payload[..payload.len().min(8000)];
        let encoded = rust_brotli(short, 11, 18, 0);
        streams.push((format!("{name} rust-brotli q11"), short.to_vec(), encoded));
    }
    streams
}

#[test]
fn push_decoder_matches_one_shot_in_any_split() {
    for (label, payload, encoded) in streams() {
        assert_eq!(burli::decompress(&encoded).unwrap(), payload, "{label}");
        for (seed, max_input, max_output) in SCHEDULES {
            if max_input.saturating_mul(max_output) < 64 && payload.len() > 60_000 {
                continue;
            }
            let (output, outcome) =
                push_decode(PushDecoder::new(), &encoded, seed, max_input, max_output);
            let schedule = format!("{label}, pieces {max_input} in, {max_output} out");
            assert_eq!(
                outcome,
                Outcome::Finished {
                    consumed: encoded.len()
                },
                "{schedule}"
            );
            assert!(output == payload, "{schedule}: wrong output");
        }
    }
}

#[test]
fn push_decoder_leaves_bytes_after_the_stream() {
    for (label, payload, mut encoded) in streams().into_iter().step_by(3) {
        let len = encoded.len();
        encoded.extend_from_slice(b"next stream");
        for (seed, max_input, max_output) in SCHEDULES.into_iter().take(4) {
            let (output, outcome) =
                push_decode(PushDecoder::new(), &encoded, seed, max_input, max_output);
            assert_eq!(outcome, Outcome::Finished { consumed: len }, "{label}");
            assert!(output == payload, "{label}: wrong output");
        }
    }
}

/// Malformed variants of a stream: bit flips spread over it, truncations,
/// and a trailing byte.
fn corruptions(stream: &[u8]) -> Vec<Vec<u8>> {
    let len = stream.len();
    let mut corrupted = Vec::new();
    for position in [
        0,
        1,
        2,
        3,
        5,
        len / 4,
        len / 3,
        len / 2,
        len * 2 / 3,
        len - 1,
    ] {
        for bit in [0, 3, 6] {
            let mut flipped = stream.to_vec();
            flipped[position.min(len - 1)] ^= 1 << bit;
            corrupted.push(flipped);
        }
    }
    for cut in [1, len / 3, len / 2, len - 1] {
        corrupted.push(stream[..cut].to_vec());
    }
    let mut trailing = stream.to_vec();
    trailing.push(0);
    corrupted.push(trailing);
    corrupted
}

#[test]
fn push_decoder_fails_alike_in_any_split() {
    const LIMIT: usize = 1 << 20;
    for (label, _, encoded) in streams().into_iter().filter(|(_, _, e)| e.len() > 8) {
        for (index, corrupted) in corruptions(&encoded).iter().enumerate() {
            let label = format!("{label} corruption {index}");
            let options = burli::decode::Options::new().with_max_output_size(LIMIT);
            let whole = push_decode(
                PushDecoder::with_options(&options),
                corrupted,
                0,
                usize::MAX,
                usize::MAX,
            );
            for (seed, max_input, max_output) in SCHEDULES.into_iter().skip(1).take(3) {
                let split = push_decode(
                    PushDecoder::with_options(&options),
                    corrupted,
                    seed,
                    max_input,
                    max_output,
                );
                assert!(
                    split == whole,
                    "{label}: pieces {max_input} in, {max_output} out end in {:?}, \
                     whole in {:?}",
                    split.1,
                    whole.1
                );
            }

            let one_shot = burli::decompress_with_limit(corrupted, LIMIT);
            match &whole.1 {
                Outcome::Finished { consumed } if *consumed == corrupted.len() => {
                    assert!(one_shot == Ok(whole.0.clone()), "{label}: {one_shot:?}");
                }
                Outcome::Finished { .. } => assert_eq!(
                    one_shot,
                    Err(BurliError::Format("trailing bytes after Brotli stream")),
                    "{label}"
                ),
                Outcome::Truncated => assert_eq!(
                    one_shot,
                    Err(BurliError::Format("unexpected end of Brotli input")),
                    "{label}"
                ),
                // A stream that ends early with nonzero padding fails on its
                // padding. The one-shot decoder first sees the bytes after it.
                Outcome::Failed(error)
                    if *error == BurliError::Format("non-zero trailing Brotli padding")
                        && one_shot
                            == Err(BurliError::Format("trailing bytes after Brotli stream")) => {}
                Outcome::Failed(error) => {
                    assert_eq!(one_shot.as_ref(), Err(error), "{label}");
                }
            }
        }
    }
}

#[test]
fn push_decoder_enforces_the_output_limit() {
    let payload = text(100_000);
    for quality in [0, 3, 5] {
        let encoded = burli::compress(&payload, quality).unwrap();
        for limit in [0, 1, 50_000, 99_999, 100_000] {
            let options = burli::decode::Options::new().with_max_output_size(limit);
            let expected = burli::decompress_with_limit(&encoded, limit);
            for (seed, max_input, max_output) in SCHEDULES.into_iter().take(4) {
                let (output, outcome) = push_decode(
                    PushDecoder::with_options(&options),
                    &encoded,
                    seed,
                    max_input,
                    max_output,
                );
                assert!(output.len() <= limit);
                match &expected {
                    Ok(decoded) => {
                        assert!(output == *decoded, "q{quality} limit {limit}: wrong output");
                        assert_eq!(
                            outcome,
                            Outcome::Finished {
                                consumed: encoded.len()
                            }
                        );
                    }
                    Err(error) => assert_eq!(outcome, Outcome::Failed(error.clone())),
                }
            }
        }
    }
}

#[test]
fn push_decoder_writes_output_within_a_meta_block() {
    // One meta-block of the whole input.
    let payload = text(300_000);
    let encoded = rust_brotli(&payload, 5, 22, 24);
    let mut decoder = PushDecoder::new();
    let mut output = vec![0; payload.len()];
    let half = encoded.len() / 2;
    let progress = decoder.decompress(&encoded[..half], &mut output).unwrap();
    assert_eq!(progress.status, DecodeStatus::NeedsInput);
    assert_eq!(progress.consumed, half);
    assert!(progress.written > payload.len() / 4, "{progress:?}");
    assert!(
        output[..progress.written] == payload[..progress.written],
        "wrong output"
    );
}

#[test]
fn push_decoder_stops_at_the_first_byte_without_room() {
    for quality in [0, 1, 5] {
        let encoded = burli::compress(b"payload", quality).unwrap();
        let mut decoder = PushDecoder::new();
        let first = decoder.decompress(&encoded, &mut []).unwrap();
        assert_eq!(
            (first.written, first.status),
            (0, DecodeStatus::NeedsOutput)
        );
        let mut output = [0; 7];
        let rest = decoder
            .decompress(&encoded[first.consumed..], &mut output)
            .unwrap();
        // The output fills exactly, and the decoder still reads the stream
        // end.
        assert_eq!(rest.status, DecodeStatus::Finished, "q{quality}");
        assert_eq!(first.consumed + rest.consumed, encoded.len());
        assert_eq!(&output[..rest.written], b"payload");
    }
}
