use std::io::Read;
#[cfg(feature = "std")]
use std::io::Write;

use burli::{BurliError, Options, Quality};

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
fn stateful_api_round_trips_stored_streams() {
    let input = b"stateful stored payload";
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
fn stream_api_round_trips_stored_streams() {
    let input = b"stream stored payload";
    let mut encoder = burli::StreamEncoder::new(Vec::new(), 0).unwrap();
    encoder.write_all(input).unwrap();
    let encoded = encoder.finish().unwrap();
    let mut decoder = burli::StreamDecoder::new(encoded.as_slice());
    let mut decoded = Vec::new();

    decoder.read_to_end(&mut decoded).unwrap();

    assert_eq!(decoded, input);
}

#[test]
fn q0_output_decodes_with_rust_brotli() {
    let input = b"rust-brotli should decode burli stored streams";
    let encoded = burli::compress(input, 0).unwrap();
    let mut decoder = rust_brotli::Decompressor::new(encoded.as_slice(), 4096);
    let mut decoded = Vec::new();

    decoder.read_to_end(&mut decoded).unwrap();
    assert_eq!(decoded, input);
}

#[test]
fn q0_to_q5_outputs_decode_with_rust_brotli() {
    let input = b"rust-brotli should decode all scoped stored qualities";

    for quality in 0..=5 {
        let encoded = burli::compress(input, quality).unwrap();
        let mut decoder = rust_brotli::Decompressor::new(encoded.as_slice(), 4096);
        let mut decoded = Vec::new();

        decoder.read_to_end(&mut decoded).unwrap();
        assert_eq!(decoded, input);
    }
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

    for quality in 1..=5 {
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

    for quality in 1..=5 {
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
fn decoder_rejects_invalid_input() {
    assert!(matches!(
        burli::decompress(b"not brotli"),
        Err(BurliError::Format(_) | BurliError::InvalidWindowBits(_))
    ));
}
