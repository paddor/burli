#![cfg(feature = "std")]

mod common;

use std::{
    ffi::c_int,
    io::{self, Read},
    panic::{self, AssertUnwindSafe},
    path::Path,
};

use common::FragmentedRead;

const DEFAULT_WINDOW_BITS: c_int = 22;
const BROTLI_MODE_GENERIC: c_int = 0;
const BROTLI_MODE_TEXT: c_int = 1;
const BROTLI_MODE_FONT: c_int = 2;
const BROTLI_PARAM_QUALITY: c_int = 1;
const BROTLI_PARAM_LGWIN: c_int = 2;
const BROTLI_OPERATION_FINISH: c_int = 2;
const BROTLI_SHARED_DICTIONARY_RAW: c_int = 0;
const BROTLI_DECODER_RESULT_ERROR: c_int = 0;
const BROTLI_DECODER_RESULT_SUCCESS: c_int = 1;
const BROTLI_DECODER_RESULT_NEEDS_MORE_INPUT: c_int = 2;
const BROTLI_DECODER_RESULT_NEEDS_MORE_OUTPUT: c_int = 3;

enum BrotliEncoderState {}
enum BrotliEncoderPreparedDictionary {}
enum BrotliDecoderState {}

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
    fn BrotliEncoderCreateInstance(
        alloc_func: *mut std::ffi::c_void,
        free_func: *mut std::ffi::c_void,
        opaque: *mut std::ffi::c_void,
    ) -> *mut BrotliEncoderState;
    fn BrotliEncoderDestroyInstance(state: *mut BrotliEncoderState);
    fn BrotliEncoderSetParameter(state: *mut BrotliEncoderState, param: c_int, value: u32)
    -> c_int;
    fn BrotliEncoderPrepareDictionary(
        dictionary_type: c_int,
        data_size: usize,
        data: *const u8,
        quality: c_int,
        alloc_func: *mut std::ffi::c_void,
        free_func: *mut std::ffi::c_void,
        opaque: *mut std::ffi::c_void,
    ) -> *mut BrotliEncoderPreparedDictionary;
    fn BrotliEncoderDestroyPreparedDictionary(dictionary: *mut BrotliEncoderPreparedDictionary);
    fn BrotliEncoderAttachPreparedDictionary(
        state: *mut BrotliEncoderState,
        dictionary: *const BrotliEncoderPreparedDictionary,
    ) -> c_int;
    fn BrotliEncoderCompressStream(
        state: *mut BrotliEncoderState,
        op: c_int,
        available_in: *mut usize,
        next_in: *mut *const u8,
        available_out: *mut usize,
        next_out: *mut *mut u8,
        total_out: *mut usize,
    ) -> c_int;
    fn BrotliEncoderIsFinished(state: *mut BrotliEncoderState) -> c_int;
    fn BrotliDecoderCreateInstance(
        alloc_func: *mut std::ffi::c_void,
        free_func: *mut std::ffi::c_void,
        opaque: *mut std::ffi::c_void,
    ) -> *mut BrotliDecoderState;
    fn BrotliDecoderDestroyInstance(state: *mut BrotliDecoderState);
    fn BrotliDecoderAttachDictionary(
        state: *mut BrotliDecoderState,
        dictionary_type: c_int,
        data_size: usize,
        data: *const u8,
    ) -> c_int;
    fn BrotliDecoderDecompressStream(
        state: *mut BrotliDecoderState,
        available_in: *mut usize,
        next_in: *mut *const u8,
        available_out: *mut usize,
        next_out: *mut *mut u8,
        total_out: *mut usize,
    ) -> c_int;
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
        for (index, input) in representative_inputs().into_iter().enumerate() {
            let encoded = burli::compress(&input, quality).unwrap();

            let decoded = c_brotli_decompress(&encoded, input.len()).unwrap_or_else(|| {
                panic!(
                    "burli q{quality} representative {index} len {} failed in C decoder",
                    input.len()
                )
            });
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
fn c_brotli_raw_dictionary_decodes_through_burli() {
    let dictionary =
        b"raw-dictionary-entry:function renderTemplate(item){return item.label + item.value;}|"
            .repeat(1024);
    let raw_dictionary = burli::decode::RawDictionary::new(&dictionary);
    let input = dictionary[128..dictionary.len() - 128].to_vec();
    let encoded = c_brotli_compress_with_raw_dictionary(&input, &dictionary, 11);

    assert!(
        encoded.len() < input.len() / 16,
        "C Brotli did not use raw dictionary enough: {} -> {}",
        input.len(),
        encoded.len()
    );
    assert!(
        burli::decompress(&encoded).is_err(),
        "dictionary-backed stream decoded without raw dictionary"
    );
    assert_eq!(
        burli::decompress_with_raw_dictionary(&encoded, &raw_dictionary).unwrap(),
        input
    );
    assert_eq!(
        burli::decompress_with_raw_dictionary_and_limit(&encoded, &raw_dictionary, input.len())
            .unwrap(),
        input
    );
    let options = burli::decode::Options::new().max_output_size(input.len());
    assert_eq!(
        burli::decompress_with_raw_dictionary_and_options(&encoded, &raw_dictionary, &options)
            .unwrap(),
        input
    );
    assert_eq!(
        burli::decompress_with_raw_dictionary_and_limit(&encoded, &raw_dictionary, input.len() - 1),
        Err(burli::BurliError::OutputLimitExceeded {
            limit: input.len() - 1,
            needed: input.len(),
        })
    );

    let mut decompressor =
        burli::Decompressor::with_raw_dictionary_and_options(raw_dictionary.clone(), &options);
    assert_eq!(decompressor.decompress(&encoded).unwrap(), input);
    decompressor.clear_raw_dictionary();
    assert!(decompressor.decompress(&encoded).is_err());
    decompressor.set_raw_dictionary(&raw_dictionary);
    assert_eq!(decompressor.decompress(&encoded).unwrap(), input);
    decompressor.clear_raw_dictionary();
    assert!(decompressor.decompress(&encoded).is_err());
    decompressor.set_raw_dictionary(&raw_dictionary);
    decompressor.reset_options(&options);
    assert_eq!(decompressor.options(), options);
    assert_eq!(decompressor.raw_dictionary(), &raw_dictionary);

    let mut appended = b"decoded:".to_vec();
    let written = decompressor
        .decompress_into(&encoded, &mut appended)
        .unwrap();
    assert_eq!(written, input.len());
    assert_eq!(&appended[b"decoded:".len()..], input);

    let mut slice = vec![0_u8; input.len()];
    let written = decompressor
        .decompress_into_slice(&encoded, &mut slice)
        .unwrap();
    assert_eq!(written, input.len());
    assert_eq!(slice, input);

    let decoded_by_c = c_brotli_decompress_with_raw_dictionary(&encoded, &dictionary, input.len())
        .expect("C Brotli failed to decode its raw-dictionary stream");
    assert_eq!(decoded_by_c, input);

    let mut stream =
        burli::StreamDecoder::with_raw_dictionary(encoded.as_slice(), raw_dictionary.clone());
    let mut streamed = Vec::new();
    stream.read_to_end(&mut streamed).unwrap();
    assert_eq!(streamed, input);

    let mut limited =
        burli::StreamDecoder::with_raw_dictionary_and_limit(encoded.as_slice(), raw_dictionary, 4);
    let mut streamed = Vec::new();
    assert_eq!(
        limited.read_to_end(&mut streamed).unwrap_err().kind(),
        io::ErrorKind::InvalidData
    );
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
                            let mutation_label =
                                format!("{label} mutation position={position} mask=0x{mask:02x}");
                            assert_matches_c_decoder_or_errors(&mutated, &input, &mutation_label);
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
        let source = FragmentedRead::new(encoded, 17);
        let mut decoder = burli::StreamDecoder::with_limit(source, max_output);
        let mut decoded = Vec::new();
        decoder.read_to_end(&mut decoded).map(|_| decoded)
    });

    match c_decoded {
        Some(expected) => {
            if is_strict_trailing_decode_error(&burli_decoded)
                && is_strict_trailing_decode_error(&sliced)
                && is_strict_trailing_decode_error(&stateful)
                && stream_matches_expected_or_strict_trailing_error(&streamed, &expected)
            {
                return;
            }

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

fn is_strict_trailing_decode_error<T>(result: &Result<T, burli::BurliError>) -> bool {
    result.as_ref().is_err_and(is_strict_trailing_burli_error)
}

fn is_strict_trailing_stream_error<T>(result: &io::Result<T>) -> bool {
    let Err(error) = result else {
        return false;
    };
    error.kind() == io::ErrorKind::InvalidData
        && error
            .get_ref()
            .and_then(|source| source.downcast_ref::<burli::BurliError>())
            .is_some_and(is_strict_trailing_burli_error)
}

fn stream_matches_expected_or_strict_trailing_error(
    result: &io::Result<Vec<u8>>,
    expected: &[u8],
) -> bool {
    match result {
        Ok(decoded) => decoded == expected,
        Err(_) => is_strict_trailing_stream_error(result),
    }
}

fn is_strict_trailing_burli_error(error: &burli::BurliError) -> bool {
    matches!(
        error,
        burli::BurliError::Format(
            "trailing bytes after Brotli stream" | "non-zero trailing Brotli padding"
        )
    )
}

fn assert_stream_decodes(encoded: &[u8], expected: &[u8], label: &str, chunk: usize) {
    let source = FragmentedRead::new(encoded, chunk);
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

fn c_brotli_compress_with_raw_dictionary(input: &[u8], dictionary: &[u8], quality: u8) -> Vec<u8> {
    unsafe {
        let state = BrotliEncoderCreateInstance(
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        );
        assert!(!state.is_null(), "C Brotli encoder allocation failed");
        assert_eq!(
            BrotliEncoderSetParameter(state, BROTLI_PARAM_QUALITY, u32::from(quality)),
            1,
            "C Brotli quality parameter failed"
        );
        assert_eq!(
            BrotliEncoderSetParameter(state, BROTLI_PARAM_LGWIN, DEFAULT_WINDOW_BITS as u32),
            1,
            "C Brotli window parameter failed"
        );

        let prepared = BrotliEncoderPrepareDictionary(
            BROTLI_SHARED_DICTIONARY_RAW,
            dictionary.len(),
            dictionary.as_ptr(),
            c_int::from(quality),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        );
        assert!(!prepared.is_null(), "C Brotli dictionary prepare failed");
        assert_eq!(
            BrotliEncoderAttachPreparedDictionary(state, prepared),
            1,
            "C Brotli dictionary attach failed"
        );

        let mut available_in = input.len();
        let mut next_in = input.as_ptr();
        let mut output = Vec::new();
        while BrotliEncoderIsFinished(state) == 0 {
            let mut chunk = [0_u8; 4096];
            let mut available_out = chunk.len();
            let mut next_out = chunk.as_mut_ptr();
            let ok = BrotliEncoderCompressStream(
                state,
                BROTLI_OPERATION_FINISH,
                &raw mut available_in,
                &raw mut next_in,
                &raw mut available_out,
                &raw mut next_out,
                std::ptr::null_mut(),
            );
            assert_eq!(ok, 1, "C Brotli dictionary compression failed");
            let produced = chunk.len() - available_out;
            output.extend_from_slice(&chunk[..produced]);
        }

        BrotliEncoderDestroyPreparedDictionary(prepared);
        BrotliEncoderDestroyInstance(state);
        output
    }
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

fn c_brotli_decompress_with_raw_dictionary(
    input: &[u8],
    dictionary: &[u8],
    decoded_size: usize,
) -> Option<Vec<u8>> {
    unsafe {
        let state = BrotliDecoderCreateInstance(
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        );
        assert!(!state.is_null(), "C Brotli decoder allocation failed");
        assert_eq!(
            BrotliDecoderAttachDictionary(
                state,
                BROTLI_SHARED_DICTIONARY_RAW,
                dictionary.len(),
                dictionary.as_ptr(),
            ),
            1,
            "C Brotli decoder dictionary attach failed"
        );

        let mut available_in = input.len();
        let mut next_in = input.as_ptr();
        let mut output = Vec::with_capacity(decoded_size);
        loop {
            let mut chunk = [0_u8; 4096];
            let mut available_out = chunk.len();
            let mut next_out = chunk.as_mut_ptr();
            let result = BrotliDecoderDecompressStream(
                state,
                &raw mut available_in,
                &raw mut next_in,
                &raw mut available_out,
                &raw mut next_out,
                std::ptr::null_mut(),
            );
            let produced = chunk.len() - available_out;
            output.extend_from_slice(&chunk[..produced]);
            match result {
                BROTLI_DECODER_RESULT_SUCCESS => {
                    BrotliDecoderDestroyInstance(state);
                    return Some(output);
                }
                BROTLI_DECODER_RESULT_NEEDS_MORE_OUTPUT => {}
                BROTLI_DECODER_RESULT_NEEDS_MORE_INPUT | BROTLI_DECODER_RESULT_ERROR => {
                    BrotliDecoderDestroyInstance(state);
                    return None;
                }
                other => panic!("unexpected C Brotli decoder result: {other}"),
            }
        }
    }
}
