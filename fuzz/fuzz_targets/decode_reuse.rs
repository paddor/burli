#![no_main]
//! A reused `Decompressor` must decode every input exactly like a fresh one,
//! even after decoding other, possibly malformed, inputs and after raw
//! dictionary changes. A `StreamDecoder` must return the same result for any
//! read sizes, keep failing after an error, and stop at the stream end.

use std::io::{self, Read};

use burli::{BurliError, Decompressor, StreamDecoder, decode::RawDictionary};
use libfuzzer_sys::fuzz_target;

const LIMIT: usize = 1 << 20;
const SLICE_LEN: usize = 1 << 12;
const DICTIONARY: &[u8] =
    b"<!doctype html><title>raw dictionary</title><p class=\"x\">0123456789abcdef</p>";

type Decoded = (
    Result<Vec<u8>, BurliError>,
    Result<usize, BurliError>,
    Vec<u8>,
    Result<usize, BurliError>,
    Vec<u8>,
);

fn decode(decompressor: &mut Decompressor, input: &[u8]) -> Decoded {
    let vec = decompressor.decompress(input);
    let mut appended = b"prefix".to_vec();
    let appended_len = decompressor.decompress_into(input, &mut appended);
    let mut slice = vec![0xa5; SLICE_LEN];
    let slice_len = decompressor.decompress_into_slice(input, &mut slice);
    (vec, appended_len, appended, slice_len, slice)
}

struct Chunked<'a> {
    input: &'a [u8],
    chunk: usize,
}

impl Read for Chunked<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let count = buf.len().min(self.chunk).min(self.input.len());
        buf[..count].copy_from_slice(&self.input[..count]);
        self.input = &self.input[count..];
        Ok(count)
    }
}

/// Returns the decoded bytes, and the bytes after the stream end or the error.
fn stream(input: &[u8], chunk: usize) -> (Vec<u8>, Result<Vec<u8>, String>) {
    let mut decoder = StreamDecoder::with_limit(Chunked { input, chunk }, LIMIT);
    let mut output = Vec::new();
    if let Err(error) = decoder.read_to_end(&mut output) {
        let retry = decoder
            .read(&mut [0; 64])
            .map_err(|error| error.to_string());
        assert_eq!(retry, Err(error.to_string()));
        return (output, Err(error.to_string()));
    }
    let unread = match decoder.into_inner() {
        Ok(inner) => inner.input.to_vec(),
        Err(error) => {
            let (inner, mut buffered) = error.into_parts();
            buffered.extend_from_slice(inner.input);
            buffered
        }
    };
    (output, Ok(unread))
}

fuzz_target!(|data: &[u8]| {
    let Some((&split, rest)) = data.split_first() else {
        return;
    };
    let (first, second) = rest.split_at(usize::from(split) * rest.len() / 255);
    let empty = RawDictionary::empty();
    let dictionary = RawDictionary::new(DICTIONARY);

    let mut reused = Decompressor::with_limit(LIMIT);
    for (input, dictionary) in [
        (first, &empty),
        (second, &empty),
        (first, &dictionary),
        (second, &dictionary),
        (first, &empty),
        (second, &dictionary),
    ] {
        if dictionary.is_empty() {
            reused.clear_raw_dictionary();
        } else {
            reused.set_raw_dictionary(dictionary);
        }
        let mut fresh = Decompressor::with_raw_dictionary_and_limit(dictionary.clone(), LIMIT);
        let expected = decode(&mut fresh, input);
        assert_eq!(decode(&mut reused, input), expected);
    }

    for input in [first, second] {
        let whole = stream(input, usize::MAX);
        assert_eq!(stream(input, input.len() / 8 + 1), whole);
    }

    if let Ok(expected) = burli::decompress_with_limit(first, LIMIT) {
        let joined = [first, second].concat();
        let (output, unread) = stream(&joined, joined.len() / 8 + 1);
        assert_eq!(output, expected);
        assert_eq!(unread.as_deref(), Ok(second));
    }
});
