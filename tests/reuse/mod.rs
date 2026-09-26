//! Reuse oracles: a reused `Decompressor` must decode like a fresh one, and a
//! `StreamDecoder` must decode alike whatever the read sizes.

#[cfg(feature = "std")]
use std::io::Read;

use burli::{BurliError, Decompressor};

#[cfg(feature = "std")]
use crate::common::FragmentedRead;

const PREFIX: &[u8] = b"prefix:";
const SLICE_LEN: usize = 16 * 1024;
const SLICE_FILL: u8 = 0xa5;

/// Results of every `Decompressor` entry point for one input, with the
/// caller buffers as they are after the call.
#[derive(PartialEq, Eq)]
struct Decoded {
    vec: Result<Vec<u8>, BurliError>,
    appended: (Result<usize, BurliError>, Vec<u8>),
    slice: (Result<usize, BurliError>, Vec<u8>),
}

impl Decoded {
    fn decode(decompressor: &mut Decompressor, input: &[u8]) -> Self {
        let vec = decompressor.decompress(input);
        let mut appended = PREFIX.to_vec();
        let appended = (decompressor.decompress_into(input, &mut appended), appended);
        let mut slice = vec![SLICE_FILL; SLICE_LEN];
        let slice = (decompressor.decompress_into_slice(input, &mut slice), slice);
        Self {
            vec,
            appended,
            slice,
        }
    }

    fn summary(&self) -> String {
        format!(
            "decompress={} decompress_into={:?} buffer={} decompress_into_slice={:?} slice={}",
            match &self.vec {
                Ok(bytes) => format!("Ok({})", digest(bytes)),
                Err(error) => format!("Err({error:?})"),
            },
            self.appended.0,
            digest(&self.appended.1),
            self.slice.0,
            digest(&self.slice.1),
        )
    }
}

/// Decodes `input` with `reused` and with `fresh`, a new decompressor with the
/// same configuration. Asserts that every entry point returns the same result
/// and leaves the caller buffers alike. Returns the `decompress` result.
pub fn assert_matches_fresh(
    reused: &mut Decompressor,
    mut fresh: Decompressor,
    input: &[u8],
    label: &str,
) -> Result<Vec<u8>, BurliError> {
    let expected = Decoded::decode(&mut fresh, input);
    let actual = Decoded::decode(reused, input);
    assert!(
        actual == expected,
        "{label}: reused decompressor diverged on {} input bytes\n reused: {}\n  fresh: {}",
        input.len(),
        actual.summary(),
        expected.summary(),
    );
    actual.vec
}

/// Reads `input` to the end with a `StreamDecoder` through reads of at most
/// `chunk` bytes. Returns the bytes read and the error, if any. A failed
/// decoder must fail again with the same error when read once more.
#[cfg(feature = "std")]
pub fn read_stream(input: &[u8], chunk: usize, limit: usize) -> (Vec<u8>, Result<(), String>) {
    let source = FragmentedRead::new(input, chunk);
    let mut decoder = burli::StreamDecoder::with_limit(source, limit);
    let mut output = Vec::new();
    let result = decoder
        .read_to_end(&mut output)
        .map(drop)
        .map_err(|error| error.to_string());
    if let Err(error) = &result {
        let retry = decoder
            .read(&mut [0; 64])
            .map_err(|error| error.to_string());
        assert_eq!(
            retry,
            Err(error.clone()),
            "chunk {chunk}: failed stream decoder did not fail again"
        );
    }
    (output, result)
}

/// Returns malformed variants of a valid stream: bit flips in the first bytes,
/// where the window bits and the first meta-block header live, bit flips
/// later in the command stream, truncations, and a trailing byte.
pub fn corruptions(stream: &[u8]) -> Vec<Vec<u8>> {
    let len = stream.len();
    let last = len.saturating_sub(1);
    let mut positions = vec![0, 1, 2, 3, len / 3, len / 2, len * 2 / 3, last];
    positions.retain(|&position| position < len);
    positions.sort_unstable();
    positions.dedup();

    let mut corrupted = Vec::new();
    for position in positions {
        for mask in [0x01, 0x80] {
            let mut flipped = stream.to_vec();
            flipped[position] ^= mask;
            corrupted.push(flipped);
        }
    }
    let mut cuts = vec![1, len / 2, last];
    cuts.retain(|&cut| cut < len);
    cuts.sort_unstable();
    cuts.dedup();
    for cut in cuts {
        corrupted.push(stream[..cut].to_vec());
    }
    let mut extended = stream.to_vec();
    extended.push(0);
    corrupted.push(extended);
    corrupted
}

fn digest(bytes: &[u8]) -> String {
    let hash = bytes.iter().fold(0xcbf2_9ce4_8422_2325_u64, |hash, &byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
    });
    format!("{} bytes, fnv {hash:016x}", bytes.len())
}
