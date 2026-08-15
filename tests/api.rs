use std::io::Read;
#[cfg(feature = "std")]
use std::io::{Cursor, Write};

use burli::{BurliError, Options, Quality};

const LOCAL_CORPUS_RATIO_SAMPLE_LIMIT: u64 = 1024 * 1024;

#[test]
fn validates_quality() {
    assert_eq!(Quality::new(0).unwrap().get(), 0);
    assert_eq!(Quality::new(11).unwrap().get(), 11);
    assert_eq!(Quality::new(12), Err(BurliError::InvalidQuality(12)));
}

#[test]
fn validates_window_bits() {
    assert!(Options::default().window_bits(10).is_ok());
    assert!(Options::default().window_bits(24).is_ok());
    assert_eq!(
        Options::default().window_bits(25),
        Err(BurliError::InvalidWindowBits(25))
    );
}

#[test]
fn decoder_handles_empty_stream() {
    assert_eq!(burli::decompress(&[0x06]).unwrap(), b"");
}

#[test]
fn q0_round_trips_through_burli() {
    let input = b"small web payload";
    let encoded = burli::compress(input, 0).unwrap();

    assert_eq!(burli::decompress(&encoded).unwrap(), input);
}

#[test]
fn q0_to_q5_round_trip_through_burli() {
    let input = b"small web payload across scoped qualities";

    for quality in 0..=5 {
        let encoded = burli::compress(input, quality).unwrap();
        assert_eq!(burli::decompress(&encoded).unwrap(), input);
    }
}

#[test]
fn q6_encoder_path_returns_unsupported() {
    assert!(matches!(
        burli::compress(b"hello", 6),
        Err(BurliError::Unsupported(_))
    ));
}

#[test]
fn stateful_api_round_trips_uncompressed_streams() {
    let input = b"stateful uncompressed payload";
    let mut compressor = burli::Compressor::new(0).unwrap();
    let mut decompressor = burli::Decompressor::new();

    let encoded = compressor.compress(input).unwrap();
    let decoded = decompressor.decompress(&encoded).unwrap();

    assert_eq!(decoded, input);
}

#[test]
fn stateful_api_appends_into_existing_buffers() {
    let input = b"append payload";
    let mut compressor = burli::Compressor::new(0).unwrap();
    let mut decompressor = burli::Decompressor::new();
    let mut encoded = b"prefix:".to_vec();
    let written = compressor.compress_into(input, &mut encoded).unwrap();
    let stream = encoded[encoded.len() - written..].to_vec();
    let mut decoded = b"decoded:".to_vec();

    let decoded_written = decompressor.decompress_into(&stream, &mut decoded).unwrap();

    assert_eq!(written, stream.len());
    assert_eq!(decoded_written, input.len());
    assert_eq!(decoded, b"decoded:append payload");
}

#[test]
fn stateful_compressor_reuses_q0_workspace_without_stale_matches() {
    let first = b"function demo(){return demo_value;} ".repeat(512);
    let second = b"abcdefghijklmnopqrstuvwxyz0123456789".repeat(257);
    let mut compressor = burli::Compressor::new(0).unwrap();

    let first_encoded = compressor.compress(&first).unwrap();
    let second_encoded = compressor.compress(&second).unwrap();

    assert_eq!(first_encoded, burli::compress(&first, 0).unwrap());
    assert_eq!(second_encoded, burli::compress(&second, 0).unwrap());
    assert_eq!(burli::decompress(&first_encoded).unwrap(), first);
    assert_eq!(burli::decompress(&second_encoded).unwrap(), second);
}

#[test]
fn stateful_q0_workspace_handles_many_reuses() {
    let inputs = [
        b"function demo(){return demo_value;} ".repeat(128),
        b"abcdefghijklmnopqrstuvwxyz0123456789".repeat(129),
    ];
    let mut compressor = burli::Compressor::new(0).unwrap();

    for index in 0..300 {
        let input = &inputs[index % inputs.len()];
        let encoded = compressor.compress(input).unwrap();

        assert_eq!(encoded, burli::compress(input, 0).unwrap());
        assert_eq!(burli::decompress(&encoded).unwrap(), *input);
    }
}

#[test]
fn decode_into_slice_writes_existing_buffer() {
    let input = b"slice payload";
    let encoded = burli::compress(input, 0).unwrap();
    let mut decoded = [0_u8; 32];

    let written = burli::decompress_into_slice(&encoded, &mut decoded).unwrap();

    assert_eq!(written, input.len());
    assert_eq!(&decoded[..written], input);
}

#[test]
fn stateful_decode_into_slice_respects_buffer_size() {
    let input = b"too large";
    let encoded = burli::compress(input, 0).unwrap();
    let mut decompressor = burli::Decompressor::new();
    let mut decoded = [0_u8; 4];

    assert_eq!(
        decompressor.decompress_into_slice(&encoded, &mut decoded),
        Err(BurliError::OutputLimitExceeded {
            limit: 4,
            needed: input.len()
        })
    );
}

#[test]
#[cfg(feature = "std")]
fn stream_api_round_trips_uncompressed_streams() {
    let input = b"stream uncompressed payload";
    let mut encoder = burli::StreamEncoder::new(Vec::new(), 0).unwrap();
    encoder.write_all(input).unwrap();
    let encoded = encoder.finish().unwrap();
    let mut decoder = burli::StreamDecoder::new(encoded.as_slice());
    let mut decoded = Vec::new();

    decoder.read_to_end(&mut decoded).unwrap();

    assert_eq!(decoded, input);
}

#[test]
#[cfg(feature = "std")]
fn stream_encoder_q5_output_decodes_with_rust_brotli() {
    let input = b"abcdefghijklmnopqrstuvwxyz0123456789".repeat(4096);
    let mut encoder = burli::StreamEncoder::new(Vec::new(), 5).unwrap();
    encoder.write_all(&input[..10_000]).unwrap();
    encoder.write_all(&input[10_000..]).unwrap();
    let encoded = encoder.finish().unwrap();
    let mut decoder = rust_brotli::Decompressor::new(encoded.as_slice(), 4096);
    let mut decoded = Vec::new();

    decoder.read_to_end(&mut decoded).unwrap();

    assert_eq!(decoded, input);
}

#[test]
#[cfg(feature = "std")]
fn stream_decoder_with_limit_reports_invalid_data() {
    let encoded = burli::compress(b"too large", 0).unwrap();
    let mut decoder = burli::StreamDecoder::with_limit(encoded.as_slice(), 4);
    let mut decoded = Vec::new();

    let error = decoder.read_to_end(&mut decoded).unwrap_err();

    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
}

#[test]
#[cfg(feature = "std")]
fn stream_decoder_empty_read_does_not_consume_input() {
    let encoded = burli::compress(b"x", 0).unwrap();
    let cursor = Cursor::new(encoded.as_slice());
    let mut decoder = burli::StreamDecoder::new(cursor);
    let mut empty = [];

    assert_eq!(decoder.read(&mut empty).unwrap(), 0);
    assert_eq!(decoder.into_inner().position(), 0);
}

#[test]
#[cfg(feature = "std")]
fn stream_decoder_emits_before_consuming_full_input() {
    let mut state = 0x1234_5678_u32;
    let input = (0..((1 << 16) * 3 + 4096))
        .map(|_| {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (state >> 24) as u8
        })
        .collect::<Vec<_>>();
    let options = Options::default()
        .quality(0)
        .unwrap()
        .block_bits(Some(16))
        .unwrap();
    let encoded = burli::compress_with_options(&input, &options).unwrap();
    let cursor = Cursor::new(encoded.as_slice());
    let mut decoder = burli::StreamDecoder::new(cursor);
    let mut decoded = [0_u8; 1];

    assert_eq!(decoder.read(&mut decoded).unwrap(), 1);

    assert_eq!(decoded[0], input[0]);
    assert!((decoder.into_inner().position() as usize) < encoded.len());
}

#[test]
fn q0_output_decodes_with_rust_brotli() {
    let input = b"rust-brotli should decode burli uncompressed streams";
    let encoded = burli::compress(input, 0).unwrap();
    let mut decoder = rust_brotli::Decompressor::new(encoded.as_slice(), 4096);
    let mut decoded = Vec::new();

    decoder.read_to_end(&mut decoded).unwrap();
    assert_eq!(decoded, input);
}

#[test]
fn q0_to_q5_outputs_decode_with_rust_brotli() {
    let input = b"rust-brotli should decode all scoped uncompressed qualities";

    for quality in 0..=5 {
        let encoded = burli::compress(input, quality).unwrap();
        let mut decoder = rust_brotli::Decompressor::new(encoded.as_slice(), 4096);
        let mut decoded = Vec::new();

        decoder.read_to_end(&mut decoded).unwrap();
        assert_eq!(decoded, input);
    }
}

#[test]
fn q5_large_output_decodes_with_rust_brotli() {
    let input = vec![b'x'; 70_000];
    let encoded = burli::compress(&input, 5).unwrap();
    let mut decoder = rust_brotli::Decompressor::new(encoded.as_slice(), 4096);
    let mut decoded = Vec::new();

    decoder.read_to_end(&mut decoded).unwrap();
    assert_eq!(decoded, input);
}

#[test]
fn q5_repeated_output_decodes_with_rust_brotli() {
    let input = b"0123456789abcdef".repeat(128);
    let encoded = burli::compress(&input, 5).unwrap();
    let mut decoder = rust_brotli::Decompressor::new(encoded.as_slice(), 4096);
    let mut decoded = Vec::new();

    assert!(encoded.len() < input.len());
    decoder.read_to_end(&mut decoded).unwrap();
    assert_eq!(decoded, input);
}

#[test]
fn q2_dictionary_after_split_meta_block_uses_global_base() {
    let mut input = vec![b'x'; 1 << 16];
    input.extend_from_slice(b"time_");
    for index in 0..4096 {
        input.push((index * 37 + 11) as u8);
    }
    let options = Options::default()
        .quality(2)
        .unwrap()
        .window_bits(10)
        .unwrap()
        .block_bits(Some(16))
        .unwrap();
    let encoded = burli::compress_with_options(&input, &options).unwrap();

    assert_eq!(burli::decompress(&encoded).unwrap(), input);
}

#[test]
fn q4_q5_dictionary_after_split_meta_block_uses_global_base() {
    let mut input = vec![b'x'; 2048];
    input.extend_from_slice(b"time time time after a split meta-block");

    for quality in 4..=5 {
        let options = Options::default()
            .quality(quality)
            .unwrap()
            .window_bits(10)
            .unwrap();
        let encoded = burli::compress_with_options(&input, &options).unwrap();

        assert_eq!(
            burli::decompress(&encoded)
                .unwrap_or_else(|error| panic!("q{quality} decode failed: {error:?}")),
            input
        );
    }
}

#[test]
#[cfg(feature = "std")]
fn stream_encoder_dictionary_after_split_meta_block_uses_global_base() {
    let mut input = vec![b'x'; 1 << 16];
    input.extend_from_slice(b"time_");
    for index in 0..4096 {
        input.push((index * 37 + 11) as u8);
    }
    let options = Options::default()
        .quality(2)
        .unwrap()
        .window_bits(10)
        .unwrap()
        .block_bits(Some(16))
        .unwrap();
    let mut encoder = burli::StreamEncoder::with_options(Vec::new(), options).unwrap();

    encoder.write_all(&input).unwrap();
    let encoded = encoder.finish().unwrap();

    assert_eq!(burli::decompress(&encoded).unwrap(), input);
}

#[test]
fn burli_decodes_rust_brotli_empty_stream() {
    let mut encoder = rust_brotli::CompressorReader::new(&b""[..], 4096, 0, 22);
    let mut encoded = Vec::new();

    encoder.read_to_end(&mut encoded).unwrap();
    assert_eq!(burli::decompress(&encoded).unwrap(), b"");
}

#[test]
fn burli_decodes_rust_brotli_small_compressed_streams() {
    let inputs: &[&[u8]] = &[
        b"abc abc abc abc",
        b"function demo(){return 42;} function demo(){return 42;}",
        b"{\"name\":\"burli\",\"kind\":\"brotli\",\"kind\":\"brotli\"}",
    ];

    for quality in 1..=11 {
        for input in inputs {
            let mut encoder = rust_brotli::CompressorReader::new(*input, 4096, quality, 22);
            let mut encoded = Vec::new();

            encoder.read_to_end(&mut encoded).unwrap();
            assert_eq!(
                burli::decompress(&encoded)
                    .unwrap_or_else(|error| panic!("q{quality} failed: {error:?}")),
                *input
            );
        }
    }
}

#[test]
fn burli_decodes_rust_brotli_representative_compressed_streams() {
    let inputs = [
        br#"{"packages":[{"name":"burli","kind":"brotli","deps":["alloc","std"]},{"name":"decode","kind":"crate","deps":["core"]}],"ok":true}"#.repeat(32),
        b"body{font-family:system-ui;margin:0}.card{display:grid;gap:12px;padding:16px;border:1px solid #ddd}.card:hover{border-color:#999}".repeat(32),
        b"function render(items){return items.map((item)=>`<li data-id=\"${item.id}\">${item.name}</li>`).join('')} export { render };".repeat(32),
    ];

    for quality in 1..=11 {
        for input in &inputs {
            let mut encoder =
                rust_brotli::CompressorReader::new(input.as_slice(), 4096, quality, 22);
            let mut encoded = Vec::new();

            encoder.read_to_end(&mut encoded).unwrap();
            assert_eq!(
                burli::decompress(&encoded)
                    .unwrap_or_else(|error| panic!("q{quality} failed: {error:?}")),
                *input
            );
        }
    }
}

#[test]
#[ignore = "uses local benchmark corpus if already downloaded"]
fn local_silesia_q4_q5_round_trip_through_burli() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("bench/corpus/silesia");
    let entries = [
        "dickens", "mozilla", "mr", "nci", "ooffice", "osdb", "reymont", "samba", "sao", "webster",
        "x-ray", "xml",
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
        for quality in 4..=5 {
            let encoded = burli::compress(&input, quality)
                .unwrap_or_else(|error| panic!("q{quality} {entry} encode failed: {error:?}"));
            let decoded = burli::decompress(&encoded)
                .unwrap_or_else(|error| panic!("q{quality} {entry} decode failed: {error:?}"));

            assert_eq!(decoded, input, "q{quality} {entry}");
        }
    }
}

#[test]
#[ignore = "uses local benchmark corpus if already downloaded"]
fn local_web_corpus_geomean_ratio_is_monotone() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("bench/corpus/web");
    assert_local_corpus_geomean_ratio_is_monotone(
        &root,
        &[
            "jquery-3.7.1.js",
            "lodash-4.17.21.js",
            "bootstrap-5.3.3.bundle.js",
            "bootstrap-5.3.3.css",
            "github-markdown-5.5.1.css",
            "normalize-8.0.1.css",
            "react-18.2.0.production.min.js",
            "preact-10.19.6.module.js",
            "vue-3.4.21.global.prod.js",
            "citm-catalog.json",
            "mdn-getting-started.html",
            "mdn-debug-example.html",
            "mdn-document-structure.html",
            "whatwg-html-source",
        ],
    );
}

#[test]
#[ignore = "uses local benchmark corpus if already downloaded"]
fn local_silesia_corpus_geomean_ratio_is_monotone() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("bench/corpus/silesia");
    assert_local_corpus_geomean_ratio_is_monotone(
        &root,
        &[
            "dickens", "mozilla", "mr", "nci", "ooffice", "osdb", "reymont", "samba", "sao",
            "webster", "x-ray", "xml",
        ],
    );
}

#[test]
fn decoder_rejects_invalid_input() {
    assert!(matches!(
        burli::decompress(b"not brotli"),
        Err(BurliError::Format(_) | BurliError::InvalidWindowBits(_))
    ));
}

fn assert_local_corpus_geomean_ratio_is_monotone(root: &std::path::Path, entries: &[&str]) {
    if !root.exists() {
        return;
    }

    let mut inputs = Vec::new();
    for entry in entries {
        let path = root.join(entry);
        let input = read_corpus_prefix(&path);
        assert!(!input.is_empty(), "empty corpus input: {}", path.display());
        inputs.push((*entry, input));
    }

    let mut previous = 0.0_f64;
    for quality in 0..=5 {
        let geomean = geomean_compression_ratio(&inputs, quality);
        assert!(
            geomean + 1e-12 >= previous,
            "{} q{quality} geomean ratio regressed: {geomean:.6} < {previous:.6}",
            root.display()
        );
        previous = geomean;
    }
}

fn geomean_compression_ratio(inputs: &[(&str, Vec<u8>)], quality: u8) -> f64 {
    let log_sum = inputs
        .iter()
        .map(|(entry, input)| {
            let encoded = burli::compress(input, quality)
                .unwrap_or_else(|error| panic!("q{quality} {entry} encode failed: {error:?}"));
            ((input.len() as f64) / (encoded.len() as f64)).ln()
        })
        .sum::<f64>();

    (log_sum / inputs.len() as f64).exp()
}

fn read_corpus_prefix(path: &std::path::Path) -> Vec<u8> {
    let file = std::fs::File::open(path)
        .unwrap_or_else(|error| panic!("failed to open {}: {error}", path.display()));
    let mut reader = file.take(LOCAL_CORPUS_RATIO_SAMPLE_LIMIT);
    let mut input = Vec::new();

    reader
        .read_to_end(&mut input)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));

    input
}
