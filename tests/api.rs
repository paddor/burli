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
fn codec_paths_return_unsupported() {
    assert!(matches!(
        burli::compress(b"hello", 5),
        Err(BurliError::Unsupported(_))
    ));
    assert!(matches!(
        burli::decompress(b"not brotli"),
        Err(BurliError::Unsupported(_))
    ));
}
