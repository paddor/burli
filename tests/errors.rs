#[cfg(feature = "std")]
mod common;

#[cfg(feature = "std")]
use std::io::{Read, Write};
use std::panic::{self, UnwindSafe};

use burli::BurliError;
#[cfg(feature = "std")]
use common::FragmentedRead;

fn assert_no_panic<T>(f: impl FnOnce() -> T + UnwindSafe) -> T {
    panic::catch_unwind(f).expect("public API panicked")
}

#[test]
fn malformed_inputs_return_errors_without_panics() {
    let cases: &[&[u8]] = &[
        b"",
        &[0x00],
        &[0xff],
        b"not brotli",
        &[0x06, 0x00],
        &[0x06, 0x01],
        &[0x21, 0x03, 0x20, 0x00, 0x08, b'h'],
    ];

    for input in cases {
        let result = assert_no_panic(|| burli::decompress(input));
        assert!(result.is_err(), "accepted malformed input: {input:?}");
    }
}

#[test]
#[cfg(feature = "std")]
fn stream_decoder_matches_one_shot_on_fuzz_empty_metadata_prefix() {
    let input = [0x0c, 0x03];
    let one_shot = burli::decompress(&input);
    let mut decoder = burli::StreamDecoder::new(input.as_slice());
    let mut streamed = Vec::new();
    let stream = decoder.read_to_end(&mut streamed);

    assert_eq!(stream.is_ok(), one_shot.is_ok());
    if let Ok(decoded) = one_shot {
        assert_eq!(streamed, decoded);
    }
}

#[test]
#[cfg(feature = "std")]
fn truncated_valid_streams_return_errors_without_panics() {
    let input =
        b"function render(items){return items.map((item)=>item.name).join(',')};".repeat(512);

    for quality in 0..=5 {
        let encoded = burli::compress(&input, quality).unwrap();
        for cut in 0..encoded.len() {
            let truncated = &encoded[..cut];

            let one_shot = assert_no_panic(|| burli::decompress(truncated));
            assert!(
                one_shot.is_err(),
                "one-shot decoder accepted truncated q{quality} stream at {cut}/{}",
                encoded.len()
            );

            let streamed = assert_no_panic(|| {
                let mut decoder = burli::StreamDecoder::new(truncated);
                let mut decoded = Vec::new();
                decoder.read_to_end(&mut decoded).map(|_| decoded)
            });
            assert!(
                streamed.is_err(),
                "stream decoder accepted truncated q{quality} stream at {cut}/{}",
                encoded.len()
            );
        }
    }
}

#[test]
fn slice_apis_report_needed_sizes_without_partial_success() {
    let input = b"body{display:grid;gap:12px}.card{padding:16px}".repeat(128);

    for quality in 0..=5 {
        let encoded = burli::compress(&input, quality).unwrap();
        let mut too_small_encoded = vec![0_u8; encoded.len() - 1];
        assert_eq!(
            burli::compress_into_slice(&input, &mut too_small_encoded, quality),
            Err(BurliError::OutputLimitExceeded {
                limit: encoded.len() - 1,
                needed: encoded.len(),
            })
        );

        let mut compressor = burli::Compressor::new(quality).unwrap();
        assert_eq!(
            compressor.compress_into_slice(&input, &mut too_small_encoded),
            Err(BurliError::OutputLimitExceeded {
                limit: encoded.len() - 1,
                needed: encoded.len(),
            })
        );

        let mut too_small_decoded = vec![0_u8; input.len() - 1];
        assert_eq!(
            burli::decompress_into_slice(&encoded, &mut too_small_decoded),
            Err(BurliError::OutputLimitExceeded {
                limit: input.len() - 1,
                needed: input.len(),
            })
        );

        let mut decompressor = burli::Decompressor::new();
        assert_eq!(
            decompressor.decompress_into_slice(&encoded, &mut too_small_decoded),
            Err(BurliError::OutputLimitExceeded {
                limit: input.len() - 1,
                needed: input.len(),
            })
        );
    }
}

#[test]
#[cfg(feature = "std")]
fn fragmented_stream_decoder_round_trips_all_scoped_qualities() {
    let inputs = [
        b"abc abc abc abc abc abc".repeat(64),
        b"{\"name\":\"burli\",\"kind\":\"brotli\",\"deps\":[\"alloc\",\"std\"]}".repeat(64),
        b"function mount(root){root.innerHTML='<main></main>';}".repeat(64),
    ];

    for quality in 0..=5 {
        for input in &inputs {
            let encoded = burli::compress(input, quality).unwrap();
            for chunk in [1, 2, 3, 5, 8, 13, 21] {
                let source = FragmentedRead::new(&encoded, chunk);
                let mut decoder = burli::StreamDecoder::new(source);
                let mut decoded = Vec::new();

                decoder.read_to_end(&mut decoded).unwrap();

                assert_eq!(decoded, *input);
            }
        }
    }
}

#[test]
#[cfg(feature = "std")]
fn fragmented_stream_encoder_round_trips_all_scoped_qualities() {
    let input = b"abcdefghijklmnopqrstuvwxyz0123456789".repeat(2048);

    for quality in 0..=5 {
        let mut encoder = burli::StreamEncoder::new(Vec::new(), quality).unwrap();
        let mut offset = 0;
        for chunk in [1, 7, 31, 257, 4096].into_iter().cycle() {
            if offset == input.len() {
                break;
            }
            let end = (offset + chunk).min(input.len());
            encoder.write_all(&input[offset..end]).unwrap();
            offset = end;
        }

        let encoded = encoder.finish().unwrap();
        assert_eq!(burli::decompress(&encoded).unwrap(), input);

        let mut decoder = rust_brotli::Decompressor::new(encoded.as_slice(), 4096);
        let mut rust_brotli_decoded = Vec::new();
        decoder.read_to_end(&mut rust_brotli_decoded).unwrap();
        assert_eq!(rust_brotli_decoded, input);
    }
}

#[cfg(feature = "std")]
mod stream_write_retries {
    use std::io::{self, Read, Write};

    struct FaultWriter {
        bytes: Vec<u8>,
        fail_at: usize,
        failures: usize,
        kind: io::ErrorKind,
        flush_failures: usize,
    }

    impl Write for FaultWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            if self.bytes.len() == self.fail_at && self.failures != 0 {
                self.failures -= 1;
                return if self.kind == io::ErrorKind::WriteZero {
                    Ok(0)
                } else {
                    Err(self.kind.into())
                };
            }
            let before_failure = if self.failures == 0 {
                usize::MAX
            } else {
                self.fail_at - self.bytes.len()
            };
            let count = buf.len().min(before_failure).min(97);
            self.bytes.extend_from_slice(&buf[..count]);
            Ok(count)
        }

        fn flush(&mut self) -> io::Result<()> {
            if self.flush_failures != 0 {
                self.flush_failures -= 1;
                return Err(io::ErrorKind::WouldBlock.into());
            }
            Ok(())
        }
    }

    fn assert_retryable(error: &io::Error) {
        assert!(matches!(
            error.kind(),
            io::ErrorKind::WouldBlock | io::ErrorKind::WriteZero
        ));
    }

    fn write_with_retries(encoder: &mut burli::StreamEncoder<FaultWriter>, mut input: &[u8]) {
        let mut failures = 0;
        while !input.is_empty() {
            match encoder.write(input) {
                Ok(count) => {
                    assert!(count > 0);
                    input = &input[count..];
                }
                Err(error) => {
                    assert_retryable(&error);
                    failures += 1;
                    assert!(failures <= 4, "writer stopped making progress");
                }
            }
        }
    }

    fn flush_with_retries(encoder: &mut burli::StreamEncoder<FaultWriter>) {
        for _ in 0..8 {
            match encoder.flush() {
                Ok(()) => return,
                Err(error) => assert_retryable(&error),
            }
        }
        panic!("flush stopped making progress");
    }

    #[test]
    fn writes_and_flushes_resume_after_partial_output() {
        for quality in 0..=5 {
            for len in [31, (1 << 16) * 2 + 31] {
                let input: Vec<_> = b"retry partial streaming writes "
                    .iter()
                    .copied()
                    .cycle()
                    .take(len)
                    .collect();
                let mut baseline = burli::StreamEncoder::new(Vec::new(), quality).unwrap();
                baseline.write_all(&input).unwrap();
                baseline.flush().unwrap();
                let expected = baseline.finish().unwrap();

                for fail_at in [0, 1, expected.len() / 2, expected.len() - 2] {
                    for kind in [
                        io::ErrorKind::WouldBlock,
                        io::ErrorKind::Interrupted,
                        io::ErrorKind::WriteZero,
                    ] {
                        let sink = FaultWriter {
                            bytes: Vec::new(),
                            fail_at,
                            failures: 2,
                            kind,
                            flush_failures: 1,
                        };
                        let mut encoder = burli::StreamEncoder::new(sink, quality).unwrap();
                        write_with_retries(&mut encoder, &input);
                        flush_with_retries(&mut encoder);
                        let sink = encoder.finish().unwrap();
                        assert_eq!(sink.failures, 0, "failure point not reached");
                        assert_eq!(
                            sink.bytes, expected,
                            "q{quality}, len {len}, offset {fail_at}, {kind:?}"
                        );
                        assert_eq!(burli::decompress(&sink.bytes).unwrap(), input);
                        let mut reference =
                            rust_brotli::Decompressor::new(sink.bytes.as_slice(), 4096);
                        let mut decoded = Vec::new();
                        reference.read_to_end(&mut decoded).unwrap();
                        assert_eq!(decoded, input);
                    }
                }
            }
        }
    }

    #[test]
    fn finish_drains_output_retained_after_failed_flush() {
        for quality in 0..=5 {
            let input = b"finish after a partial flush";
            let sink = FaultWriter {
                bytes: Vec::new(),
                fail_at: 1,
                failures: 1,
                kind: io::ErrorKind::WouldBlock,
                flush_failures: 0,
            };
            let mut encoder = burli::StreamEncoder::new(sink, quality).unwrap();
            encoder.write_all(input).unwrap();
            assert_eq!(
                encoder.flush().unwrap_err().kind(),
                io::ErrorKind::WouldBlock
            );
            let sink = encoder.finish().unwrap();
            assert_eq!(burli::decompress(&sink.bytes).unwrap(), input);
        }
    }
}
