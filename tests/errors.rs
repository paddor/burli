#[cfg(feature = "std")]
use std::io::{self, Read, Write};
use std::panic::{self, UnwindSafe};

use burli::BurliError;

fn assert_no_panic<T>(f: impl FnOnce() -> T + UnwindSafe) -> T {
    panic::catch_unwind(f).expect("public API panicked")
}

#[cfg(feature = "std")]
struct FragmentedRead<'a> {
    input: &'a [u8],
    pos: usize,
    chunk: usize,
}

#[cfg(feature = "std")]
impl<'a> FragmentedRead<'a> {
    const fn new(input: &'a [u8], chunk: usize) -> Self {
        Self {
            input,
            pos: 0,
            chunk,
        }
    }
}

#[cfg(feature = "std")]
impl Read for FragmentedRead<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.pos == self.input.len() {
            return Ok(0);
        }
        let count = buf.len().min(self.chunk).min(self.input.len() - self.pos);
        buf[..count].copy_from_slice(&self.input[self.pos..self.pos + count]);
        self.pos += count;
        Ok(count)
    }
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

        let mut too_small_decoded = vec![0_u8; input.len() - 1];
        assert_eq!(
            burli::decompress_into_slice(&encoded, &mut too_small_decoded),
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
