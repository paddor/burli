use std::io::Read;

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
fn encoder_path_returns_unsupported() {
    assert!(matches!(
        burli::compress(b"hello", 5),
        Err(BurliError::Unsupported(_))
    ));
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
fn q0_output_decodes_with_rust_brotli() {
    let input = b"rust-brotli should decode burli stored streams";
    let encoded = burli::compress(input, 0).unwrap();
    let mut decoder = rust_brotli::Decompressor::new(encoded.as_slice(), 4096);
    let mut decoded = Vec::new();

    decoder.read_to_end(&mut decoded).unwrap();
    assert_eq!(decoded, input);
}

#[test]
fn burli_decodes_rust_brotli_empty_stream() {
    let mut encoder = rust_brotli::CompressorReader::new(&b""[..], 4096, 0, 22);
    let mut encoded = Vec::new();

    encoder.read_to_end(&mut encoded).unwrap();
    assert_eq!(burli::decompress(&encoded).unwrap(), b"");
}

#[test]
fn decoder_rejects_invalid_input() {
    assert!(matches!(
        burli::decompress(b"not brotli"),
        Err(BurliError::Format(_) | BurliError::InvalidWindowBits(_))
    ));
}
