use std::ffi::c_int;
use std::path::Path;

const DEFAULT_WINDOW_BITS: c_int = 22;
const BROTLI_MODE_GENERIC: c_int = 0;
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

fn c_brotli_compress(input: &[u8], quality: u8) -> Vec<u8> {
    let mut output_size = unsafe { BrotliEncoderMaxCompressedSize(input.len()) };
    let mut output = vec![0; output_size];
    let ok = unsafe {
        BrotliEncoderCompress(
            c_int::from(quality),
            DEFAULT_WINDOW_BITS,
            BROTLI_MODE_GENERIC,
            input.len(),
            input.as_ptr(),
            &mut output_size,
            output.as_mut_ptr(),
        )
    };
    assert_eq!(ok, 1, "C Brotli compression failed");
    output.truncate(output_size);
    output
}

fn c_brotli_decompress(input: &[u8], decoded_size: usize) -> Option<Vec<u8>> {
    let mut output_size = decoded_size;
    let mut output = vec![0; output_size];
    let result = unsafe {
        BrotliDecoderDecompress(
            input.len(),
            input.as_ptr(),
            &mut output_size,
            output.as_mut_ptr(),
        )
    };
    if result != BROTLI_DECODER_RESULT_SUCCESS {
        return None;
    }
    output.truncate(output_size);
    Some(output)
}
