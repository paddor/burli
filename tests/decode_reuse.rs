//! Decoder reuse and boundary tests. A reused `Decompressor` must decode every
//! input like a fresh one, whatever it decoded before. A `StreamDecoder` must
//! not carry state past a partial read, a failed read, or the stream end.

#[cfg(feature = "std")]
mod common;
mod reuse;

#[cfg(feature = "std")]
use std::io::{Read, Write};

use burli::{Decompressor, Options, decode::RawDictionary};
#[cfg(feature = "std")]
use common::FragmentedRead;

const LIMIT: usize = 1 << 20;

/// Small deterministic payloads: empty, one byte, text made of dictionary
/// words, noise that stays uncompressed, and short-distance repeats.
fn payloads() -> Vec<Vec<u8>> {
    const WORDS: [&[u8]; 12] = [
        b"the ",
        b"of ",
        b"and ",
        b"brotli ",
        b"window ",
        b"dictionary ",
        b"stream ",
        b"decoder ",
        b"context ",
        b"<div class=\"card\">",
        b"function ",
        b"return ",
    ];
    let mut state = 0x1234_5678_u32;
    let mut next = move || {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (state >> 24) as usize
    };
    let text = (0..400)
        .flat_map(|_| WORDS[next() % WORDS.len()].iter().copied())
        .collect();
    let noise = (0..1500).map(|_| next() as u8).collect();
    vec![
        Vec::new(),
        b"a".to_vec(),
        text,
        noise,
        b"abcdefgh".repeat(300),
    ]
}

/// Valid streams with their payloads: every payload at q0..q5, the same at
/// q1..q5 with a 1 KiB window, and flushed streams with many meta-blocks.
fn streams() -> Vec<(Vec<u8>, Vec<u8>)> {
    let payloads = payloads();
    let mut streams = Vec::new();
    for quality in 0..=5 {
        for payload in &payloads {
            streams.push((payload.clone(), burli::compress(payload, quality).unwrap()));
            // The q0 match finders for inputs up to 64 KiB do not check the
            // window size, so q0 streams with small windows can be invalid.
            if quality != 0 {
                let small_window = Options::default()
                    .with_quality(quality)
                    .unwrap()
                    .with_window_bits(10)
                    .unwrap();
                let encoded = burli::compress_with_options(payload, &small_window).unwrap();
                streams.push((payload.clone(), encoded));
            }
        }
        #[cfg(feature = "std")]
        {
            let payload = payloads[2..].concat();
            let mut encoder = burli::StreamEncoder::new(Vec::new(), quality).unwrap();
            for chunk in payload.chunks(700) {
                encoder.write_all(chunk).unwrap();
                encoder.flush().unwrap();
            }
            streams.push((payload, encoder.finish().unwrap()));
        }
    }
    streams
}

#[test]
fn reused_decompressor_matches_fresh_after_malformed_streams() {
    let mut reused = Decompressor::with_limit(LIMIT);
    let mut failures = 0_usize;

    for (index, (payload, stream)) in streams().iter().enumerate() {
        for (corruption, corrupted) in reuse::corruptions(stream).iter().enumerate() {
            let label = format!("stream {index} corruption {corruption}");
            let fresh = Decompressor::with_limit(LIMIT);
            let result = reuse::assert_matches_fresh(&mut reused, fresh, corrupted, &label);
            failures += usize::from(result.is_err());

            let label = format!("stream {index} after corruption {corruption}");
            let fresh = Decompressor::with_limit(LIMIT);
            let decoded = reuse::assert_matches_fresh(&mut reused, fresh, stream, &label);
            assert!(decoded.as_ref() == Ok(payload), "{label}: wrong payload");
        }
    }

    assert!(failures > 0, "no corrupted stream failed to decode");
}

#[test]
fn reused_decompressor_matches_fresh_across_repeated_streams() {
    let streams = streams();
    let mut reused = Decompressor::with_limit(LIMIT);

    for round in 0..3 {
        for (index, (payload, stream)) in streams.iter().enumerate() {
            for repeat in 0..2 {
                let label = format!("round {round} stream {index} repeat {repeat}");
                let fresh = Decompressor::with_limit(LIMIT);
                let decoded = reuse::assert_matches_fresh(&mut reused, fresh, stream, &label);
                assert!(decoded.as_ref() == Ok(payload), "{label}: wrong payload");
            }
        }
    }
}

#[test]
fn reused_decompressor_matches_fresh_across_dictionary_changes() {
    // Burli does not encode with raw dictionaries, but a raw dictionary shifts
    // the static dictionary distance base, so some streams decode differently
    // or fail with one attached.
    let dictionaries = [
        RawDictionary::new(b"raw dictionary bytes "),
        RawDictionary::empty(),
        RawDictionary::new(&b"0123456789abcdef".repeat(64)),
    ];
    let mut reused = Decompressor::with_limit(LIMIT);
    let mut dictionary_changed_output = false;

    for (index, (payload, stream)) in streams().iter().enumerate() {
        let mut inputs = reuse::corruptions(stream);
        inputs.push(stream.clone());
        for (input_index, input) in inputs.iter().enumerate() {
            for (dictionary_index, dictionary) in dictionaries.iter().enumerate() {
                if dictionary.is_empty() {
                    reused.clear_raw_dictionary();
                } else {
                    reused.set_raw_dictionary(dictionary);
                }
                let label =
                    format!("stream {index} input {input_index} dictionary {dictionary_index}");
                let fresh = Decompressor::with_raw_dictionary_and_limit(dictionary.clone(), LIMIT);
                let decoded = reuse::assert_matches_fresh(&mut reused, fresh, input, &label);
                if input == stream {
                    dictionary_changed_output |= decoded.as_ref() != Ok(payload);
                }
            }
        }
    }

    assert!(
        dictionary_changed_output,
        "no valid stream depended on the raw dictionary"
    );
}

#[test]
fn reused_decompressor_matches_fresh_across_limit_changes() {
    let mut reused = Decompressor::with_limit(LIMIT);

    for (index, (payload, stream)) in streams().iter().enumerate() {
        for (step, limit) in [payload.len().saturating_sub(1), LIMIT, payload.len()]
            .into_iter()
            .enumerate()
        {
            if step % 2 == 0 {
                reused.set_limit(limit);
            } else {
                reused.reset_options(&burli::decode::Options::new().with_max_output_size(limit));
            }
            let label = format!("stream {index} limit {limit}");
            let fresh = Decompressor::with_limit(limit);
            let decoded = reuse::assert_matches_fresh(&mut reused, fresh, stream, &label);
            assert_eq!(decoded.is_ok(), payload.len() <= limit, "{label}");
        }
    }
}

#[test]
#[cfg(feature = "std")]
fn stream_decoder_reads_malformed_streams_alike_in_any_fragmentation() {
    for (index, (_, stream)) in streams().iter().enumerate() {
        for (corruption, corrupted) in reuse::corruptions(stream).iter().enumerate() {
            let label = format!("stream {index} corruption {corruption}");
            let whole = reuse::read_stream(corrupted, usize::MAX, LIMIT);
            if let Ok(decoded) = burli::decompress_with_limit(corrupted, LIMIT) {
                assert!(
                    whole == (decoded, Ok(())),
                    "{label}: stream and one-shot differ"
                );
            }
            for chunk in [7, 64] {
                assert!(
                    reuse::read_stream(corrupted, chunk, LIMIT) == whole,
                    "{label}: chunk {chunk} differs from whole reads"
                );
            }
        }
    }
}

#[test]
#[cfg(feature = "std")]
fn stream_decoder_stops_at_the_stream_boundary() {
    let streams = streams();

    for (index, pair) in streams.windows(2).enumerate() {
        let [(first_payload, first), (second_payload, second)] = pair else {
            unreachable!();
        };
        let joined = [first.as_slice(), second.as_slice()].concat();
        for chunk in [2, 13, usize::MAX] {
            let label = format!("streams {index} and {} chunk {chunk}", index + 1);
            let mut decoder =
                burli::StreamDecoder::with_limit(FragmentedRead::new(&joined, chunk), LIMIT);
            let mut output = Vec::new();
            decoder
                .read_to_end(&mut output)
                .unwrap_or_else(|error| panic!("{label}: {error}"));
            assert!(output == *first_payload, "{label}: wrong first payload");

            let (inner, mut unread) = match decoder.into_inner() {
                Ok(inner) => (inner, Vec::new()),
                Err(error) => error.into_parts(),
            };
            unread.extend_from_slice(&inner.input[inner.pos..]);
            assert!(unread == *second, "{label}: wrong unread bytes");
            assert!(
                reuse::read_stream(&unread, chunk, LIMIT) == (second_payload.clone(), Ok(())),
                "{label}: wrong second payload"
            );
        }
    }
}
