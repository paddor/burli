#![cfg(feature = "std")]

use std::{
    ffi::c_int,
    io::{self, Read},
    panic::{self, AssertUnwindSafe},
    path::Path,
};

const DEFAULT_WINDOW_BITS: c_int = 22;
const BROTLI_MODE_GENERIC: c_int = 0;
const BROTLI_MODE_TEXT: c_int = 1;
const BROTLI_MODE_FONT: c_int = 2;
const BROTLI_DECODER_RESULT_SUCCESS: c_int = 1;

#[link(name = "brotlienc")]
#[link(name = "brotlidec")]
#[link(name = "brotlicommon")]
unsafe extern "C" {
    fn BrotliEncoderMaxCompressedSize(input_size: usize) -> usize;
    fn BrotliEncoderCompress(
        quality: c_int,
        lgwin: c_int,
        mode: c_int,
        input_size: usize,
        input_buffer: *const u8,
        encoded_size: *mut usize,
        encoded_buffer: *mut u8,
    ) -> c_int;
    fn BrotliDecoderDecompress(
        encoded_size: usize,
        encoded_buffer: *const u8,
        decoded_size: *mut usize,
        decoded_buffer: *mut u8,
    ) -> c_int;
}

struct FragmentedRead<'a> {
    input: &'a [u8],
    pos: usize,
    chunk: usize,
}

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
fn c_brotli_q0_to_q5_decode_through_burli() {
    for quality in 0..=5 {
        for input in representative_inputs() {
            let encoded = c_brotli_compress(&input, quality);

            let decoded = burli::decompress(&encoded)
                .unwrap_or_else(|error| panic!("C q{quality} failed: {error:?}"));
            assert_bytes_eq(&decoded, &input, &format!("C q{quality} representative"));
        }
    }
}

#[test]
fn burli_q0_to_q5_decode_through_c_brotli() {
    for quality in 0..=5 {
        for input in representative_inputs() {
            let encoded = burli::compress(&input, quality).unwrap();

            let decoded = c_brotli_decompress(&encoded, input.len())
                .unwrap_or_else(|| panic!("burli q{quality} failed in C decoder"));
            assert_bytes_eq(
                &decoded,
                &input,
                &format!("burli q{quality} representative"),
            );
        }
    }
}

#[test]
fn c_brotli_q0_to_q5_web_fixture_slices_decode_through_burli() {
    for quality in 0..=5 {
        for input in web_fixture_slices() {
            let encoded = c_brotli_compress(&input, quality);
            let decoded = burli::decompress(&encoded)
                .unwrap_or_else(|error| panic!("C q{quality} web slice failed: {error:?}"));

            assert_bytes_eq(&decoded, &input, &format!("C q{quality} web slice"));
        }
    }
}

#[test]
#[ignore = "generated C Brotli q0..q11/mode/window decoder matrix"]
fn c_brotli_parameter_matrix_decodes_through_burli() {
    for (name, input) in generated_conformance_inputs() {
        for quality in 0..=11 {
            for mode in BROTLI_MODES {
                for lgwin in BROTLI_WINDOWS {
                    let encoded = c_brotli_compress_with(&input, quality, mode, lgwin);
                    let label =
                        format!("C q{quality} mode={} lgwin={lgwin} {name}", mode_name(mode));

                    assert_burli_decodes(&encoded, &input, &label);
                    assert_stream_decodes(&encoded, &input, &label, 1);
                    assert_stream_decodes(&encoded, &input, &label, 17);
                    assert_stream_decodes(&encoded, &input, &label, 4096);
                }
            }
        }
    }
}

#[test]
#[ignore = "generated C Brotli malformed-stream decoder matrix"]
fn c_brotli_truncated_and_mutated_streams_match_or_error_without_panics() {
    for (name, input) in generated_conformance_inputs() {
        for quality in [0, 1, 5, 9, 11] {
            for mode in BROTLI_MODES {
                for lgwin in [10, 16, 22] {
                    let encoded = c_brotli_compress_with(&input, quality, mode, lgwin);
                    let label =
                        format!("C q{quality} mode={} lgwin={lgwin} {name}", mode_name(mode));

                    for cut in truncation_cuts(encoded.len()) {
                        assert_decode_error_without_panic(&encoded[..cut], &label);
                    }

                    for position in mutation_positions(encoded.len()) {
                        for mask in [0x01, 0x55, 0xff] {
                            let mut mutated = encoded.clone();
                            mutated[position] ^= mask;
                            assert_matches_c_decoder_or_errors(&mutated, &input, &label);
                        }
                    }
                }
            }
        }
    }
}

#[test]
#[ignore = "uses local benchmark corpus if already downloaded"]
fn local_web_corpus_c_brotli_q0_to_q5_decode_through_burli() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("bench/corpus/web");
    let entries = [
        "bootstrap-5.3.3.bundle.js",
        "bootstrap-5.3.3.css",
        "citm-catalog.json",
        "jquery-3.7.1.js",
        "whatwg-html-source",
    ];
    if !root.exists() {
        return;
    }

    for entry in entries {
        let path = root.join(entry);
        if !path.exists() {
            continue;
        }
        let input = std::fs::read(&path).unwrap();
        for quality in 0..=5 {
            let encoded = c_brotli_compress(&input, quality);

            let decoded = burli::decompress(&encoded)
                .unwrap_or_else(|error| panic!("C q{quality} {entry} failed: {error:?}"));
            assert_bytes_eq(&decoded, &input, &format!("C q{quality} {entry}"));
        }
    }
}

#[test]
#[ignore = "uses local benchmark corpus if already downloaded"]
fn local_web_corpus_c_brotli_q0_to_q11_decode_through_burli() {
    for (entry, input) in local_web_corpus_representative_inputs() {
        for quality in 0..=11 {
            let encoded = c_brotli_compress(&input, quality);

            let decoded = burli::decompress(&encoded)
                .unwrap_or_else(|error| panic!("C q{quality} {entry} failed: {error:?}"));
            assert_bytes_eq(&decoded, &input, &format!("C q{quality} {entry}"));
        }
    }
}

#[test]
#[ignore = "uses local benchmark corpus if already downloaded"]
fn local_web_corpus_rust_brotli_q0_to_q11_decode_through_burli() {
    for (entry, input) in local_web_corpus_representative_inputs() {
        for quality in 0..=11 {
            let encoded = rust_brotli_compress(&input, quality);

            let decoded = burli::decompress(&encoded)
                .unwrap_or_else(|error| panic!("rust-brotli q{quality} {entry} failed: {error:?}"));
            assert_bytes_eq(&decoded, &input, &format!("rust-brotli q{quality} {entry}"));
        }
    }
}

fn assert_bytes_eq(actual: &[u8], expected: &[u8], label: &str) {
    if actual == expected {
        return;
    }

    let first_diff = actual
        .iter()
        .zip(expected)
        .position(|(left, right)| left != right)
        .unwrap_or_else(|| actual.len().min(expected.len()));
    panic!(
        "{label} mismatch: actual_len={}, expected_len={}, first_diff={first_diff}, actual_byte={:?}, expected_byte={:?}",
        actual.len(),
        expected.len(),
        actual.get(first_diff),
        expected.get(first_diff)
    );
}

const BROTLI_MODES: [c_int; 3] = [BROTLI_MODE_GENERIC, BROTLI_MODE_TEXT, BROTLI_MODE_FONT];
const BROTLI_WINDOWS: [c_int; 11] = [10, 11, 12, 13, 14, 15, 16, 18, 20, 22, 24];

fn mode_name(mode: c_int) -> &'static str {
    match mode {
        BROTLI_MODE_GENERIC => "generic",
        BROTLI_MODE_TEXT => "text",
        BROTLI_MODE_FONT => "font",
        _ => "unknown",
    }
}

fn assert_burli_decodes(encoded: &[u8], expected: &[u8], label: &str) {
    let decoded = burli::decompress_with_limit(encoded, expected.len())
        .unwrap_or_else(|error| panic!("{label} one-shot failed: {error:?}"));
    assert_bytes_eq(&decoded, expected, label);

    let mut slice = vec![0_u8; expected.len()];
    let written = burli::decompress_into_slice(encoded, &mut slice)
        .unwrap_or_else(|error| panic!("{label} slice failed: {error:?}"));
    assert_eq!(written, expected.len(), "{label} slice length mismatch");
    assert_bytes_eq(&slice, expected, label);

    let mut decompressor = burli::Decompressor::with_limit(expected.len());
    let decoded = decompressor
        .decompress(encoded)
        .unwrap_or_else(|error| panic!("{label} stateful failed: {error:?}"));
    assert_bytes_eq(&decoded, expected, label);
}

fn assert_no_panic<T>(action: impl FnOnce() -> T) -> T {
    panic::catch_unwind(AssertUnwindSafe(action)).expect("public decode API panicked")
}

fn assert_decode_error_without_panic(encoded: &[u8], label: &str) {
    const LIMIT: usize = 64 * 1024;

    let one_shot = assert_no_panic(|| burli::decompress_with_limit(encoded, LIMIT));
    assert!(
        one_shot.is_err(),
        "{label} truncated one-shot decode succeeded"
    );

    let mut slice = vec![0_u8; LIMIT];
    let sliced = assert_no_panic(|| burli::decompress_into_slice(encoded, &mut slice));
    assert!(sliced.is_err(), "{label} truncated slice decode succeeded");

    let mut decompressor = burli::Decompressor::with_limit(LIMIT);
    let stateful = assert_no_panic(|| decompressor.decompress(encoded));
    assert!(
        stateful.is_err(),
        "{label} truncated stateful decode succeeded"
    );

    let streamed = assert_no_panic(|| {
        let mut decoder = burli::StreamDecoder::with_limit(encoded, LIMIT);
        let mut decoded = Vec::new();
        decoder.read_to_end(&mut decoded).map(|_| decoded)
    });
    assert!(
        streamed.is_err(),
        "{label} truncated stream decode succeeded"
    );
}

fn assert_matches_c_decoder_or_errors(encoded: &[u8], input: &[u8], label: &str) {
    let max_output = input.len().saturating_mul(4).saturating_add(1024);
    let c_decoded = c_brotli_decompress(encoded, max_output);

    let burli_decoded = assert_no_panic(|| burli::decompress_with_limit(encoded, max_output));
    let mut slice = vec![0_u8; max_output];
    let sliced = assert_no_panic(|| burli::decompress_into_slice(encoded, &mut slice));
    let mut decompressor = burli::Decompressor::with_limit(max_output);
    let stateful = assert_no_panic(|| decompressor.decompress(encoded));
    let streamed = assert_no_panic(|| {
        let source = FragmentedRead {
            input: encoded,
            pos: 0,
            chunk: 17,
        };
        let mut decoder = burli::StreamDecoder::with_limit(source, max_output);
        let mut decoded = Vec::new();
        decoder.read_to_end(&mut decoded).map(|_| decoded)
    });

    match c_decoded {
        Some(expected) => {
            let burli_decoded = burli_decoded.unwrap_or_else(|error| {
                panic!("{label} mutated decode failed while C accepted it: {error:?}")
            });
            assert_bytes_eq(&burli_decoded, &expected, label);

            let written = sliced.unwrap_or_else(|error| {
                panic!("{label} mutated slice failed while C accepted it: {error:?}")
            });
            assert_eq!(written, expected.len(), "{label} mutated slice length");
            assert_bytes_eq(&slice[..written], &expected, label);

            let stateful = stateful.unwrap_or_else(|error| {
                panic!("{label} mutated stateful failed while C accepted it: {error:?}")
            });
            assert_bytes_eq(&stateful, &expected, label);

            let streamed = streamed.unwrap_or_else(|error| {
                panic!("{label} mutated stream failed while C accepted it: {error:?}")
            });
            assert_bytes_eq(&streamed, &expected, label);
        }
        None => {
            assert!(
                burli_decoded.is_err(),
                "{label} mutated one-shot succeeded while C rejected it"
            );
            assert!(
                sliced.is_err(),
                "{label} mutated slice succeeded while C rejected it"
            );
            assert!(
                stateful.is_err(),
                "{label} mutated stateful succeeded while C rejected it"
            );
            assert!(
                streamed.is_err(),
                "{label} mutated stream succeeded while C rejected it"
            );
        }
    }
}

fn assert_stream_decodes(encoded: &[u8], expected: &[u8], label: &str, chunk: usize) {
    let source = FragmentedRead {
        input: encoded,
        pos: 0,
        chunk,
    };
    let mut decoder = burli::StreamDecoder::with_limit(source, expected.len());
    let mut decoded = Vec::new();

    decoder
        .read_to_end(&mut decoded)
        .unwrap_or_else(|error| panic!("{label} chunk {chunk} failed: {error:?}"));
    assert_bytes_eq(&decoded, expected, label);
}

fn truncation_cuts(len: usize) -> Vec<usize> {
    if len <= 512 {
        return (0..len).collect();
    }

    let mut cuts = vec![0, 1, len / 4, len / 2, len * 3 / 4, len - 1];
    cuts.sort_unstable();
    cuts.dedup();
    cuts
}

fn mutation_positions(len: usize) -> Vec<usize> {
    if len == 0 {
        return Vec::new();
    }

    let mut positions = vec![0, len / 4, len / 2, len * 3 / 4, len - 1];
    positions.sort_unstable();
    positions.dedup();
    positions
}

fn representative_inputs() -> Vec<Vec<u8>> {
    vec![
        Vec::new(),
        b"small web payload".to_vec(),
        br#"{"packages":[{"name":"burli","kind":"brotli","deps":["alloc","std"]},{"name":"decode","kind":"crate","deps":["core"]}],"ok":true}"#
            .repeat(32),
        b"body{font-family:system-ui;margin:0}.card{display:grid;gap:12px;padding:16px;border:1px solid #ddd}.card:hover{border-color:#999}"
            .repeat(32),
        b"function render(items){return items.map((item)=>`<li data-id=\"${item.id}\">${item.name}</li>`).join('')} export { render };"
            .repeat(32),
        b"abcdefghijklmnopqrstuvwxyz0123456789".repeat(4096),
    ]
}

fn generated_conformance_inputs() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("empty", Vec::new()),
        ("tiny", b"a".to_vec()),
        (
            "html",
            br#"<!doctype html><main class="card"><h1>Burli</h1><p data-id="42">decode conformance</p></main>"#
                .repeat(16),
        ),
        (
            "css",
            br".card{display:grid;grid-template-columns:repeat(auto-fit,minmax(16rem,1fr));gap:1rem;color:#1b1f23}"
                .repeat(32),
        ),
        (
            "json",
            br#"{"items":[{"name":"burli","kind":"brotli","ok":true},{"name":"decoder","kind":"crate","ok":true}]}"#
                .repeat(32),
        ),
        (
            "js",
            br#"export function render(items){return items.map((item)=>`<li data-id="${item.id}">${item.name}</li>`).join("")}"#
                .repeat(32),
        ),
        (
            "binary",
            (0..4096).map(|index| (index * 37 % 251) as u8).collect(),
        ),
    ]
}

fn web_fixture_slices() -> Vec<Vec<u8>> {
    vec![
        br#"<!doctype html>
<html lang="en">
<head><meta charset="utf-8"><title>Burli fixture</title></head>
<body><main class="stack"><h1>Compression</h1><p data-id="42">web payloads mix tags, attrs, text, and whitespace.</p></main></body>
</html>"#
            .repeat(12),
        br".btn{display:inline-flex;align-items:center;gap:.5rem;border:1px solid #0d6efd;background:#fff;color:#0d6efd}.btn:hover{background:#0d6efd;color:#fff}.grid{display:grid;grid-template-columns:repeat(auto-fit,minmax(16rem,1fr));gap:1rem}"
            .repeat(24),
        br#"export function render(items){return items.map((item,index)=>`<li data-index="${index}" data-id="${item.id}">${item.name}</li>`).join("")}const state={ready:true,count:42,items:["alpha","beta","gamma"]};"#
            .repeat(24),
        br#"{"scripts":{"build":"vite build","test":"cargo test --workspace"},"dependencies":{"@vitejs/plugin-react":"latest","typescript":"latest"},"browserslist":[">0.2%","not dead","not op_mini all"]}"#
            .repeat(24),
    ]
}

fn local_web_corpus_representative_inputs() -> Vec<(String, Vec<u8>)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("bench/corpus/web");
    let entries = [
        "bootstrap-5.3.3.css",
        "citm-catalog.json",
        "jquery-3.7.1.js",
        "mdn-document-structure.html",
    ];
    if !root.exists() {
        return Vec::new();
    }

    entries
        .into_iter()
        .filter_map(|entry| {
            let path = root.join(entry);
            path.exists()
                .then(|| (entry.to_owned(), std::fs::read(path).unwrap()))
        })
        .collect()
}

fn c_brotli_compress(input: &[u8], quality: u8) -> Vec<u8> {
    c_brotli_compress_with(input, quality, BROTLI_MODE_GENERIC, DEFAULT_WINDOW_BITS)
}

fn c_brotli_compress_with(input: &[u8], quality: u8, mode: c_int, lgwin: c_int) -> Vec<u8> {
    let mut output_size = unsafe { BrotliEncoderMaxCompressedSize(input.len()) };
    let mut output = vec![0; output_size];
    let ok = unsafe {
        BrotliEncoderCompress(
            c_int::from(quality),
            lgwin,
            mode,
            input.len(),
            input.as_ptr(),
            &raw mut output_size,
            output.as_mut_ptr(),
        )
    };
    assert_eq!(
        ok,
        1,
        "C Brotli compression failed for q{quality} mode={} lgwin={lgwin}",
        mode_name(mode)
    );
    output.truncate(output_size);
    output
}

fn rust_brotli_compress(input: &[u8], quality: u32) -> Vec<u8> {
    let mut encoder = rust_brotli::CompressorReader::new(input, 4096, quality, 22);
    let mut output = Vec::new();
    encoder.read_to_end(&mut output).unwrap();
    output
}

fn c_brotli_decompress(input: &[u8], decoded_size: usize) -> Option<Vec<u8>> {
    let mut output_size = decoded_size;
    let mut output = vec![0; output_size];
    let result = unsafe {
        BrotliDecoderDecompress(
            input.len(),
            input.as_ptr(),
            &raw mut output_size,
            output.as_mut_ptr(),
        )
    };
    if result != BROTLI_DECODER_RESULT_SUCCESS {
        return None;
    }
    output.truncate(output_size);
    Some(output)
}
