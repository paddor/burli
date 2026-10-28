#![no_main]

use burli::{BurliError, DecodeStatus, PushDecoder};
use libfuzzer_sys::fuzz_target;

#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    Finished { consumed: usize },
    Truncated,
    Failed(BurliError),
}

/// Decodes `encoded` with input and output pieces cycling through `sizes`.
fn push_decode(encoded: &[u8], sizes: &[u8]) -> (Vec<u8>, Outcome) {
    let options = burli::decode::Options::new().with_max_output_size(1 << 24);
    let mut decoder = PushDecoder::with_options(&options);
    let mut output = Vec::new();
    let mut room = [0_u8; 256];
    let mut pos = 0;
    for step in 0.. {
        let (input_size, output_size) = if sizes.is_empty() {
            (usize::MAX, room.len())
        } else {
            let size = sizes[step % sizes.len()];
            (usize::from(size % 64) + 1, usize::from(size / 64) * 85 + 1)
        };
        let end = pos + input_size.min(encoded.len() - pos);
        let progress = match decoder.decompress(&encoded[pos..end], &mut room[..output_size]) {
            Ok(progress) => progress,
            Err(error) => return (output, Outcome::Failed(error)),
        };
        assert!(progress.consumed <= end - pos);
        output.extend_from_slice(&room[..progress.written]);
        pos += progress.consumed;
        match progress.status {
            DecodeStatus::Finished => return (output, Outcome::Finished { consumed: pos }),
            DecodeStatus::NeedsInput => {
                assert_eq!(pos, end);
                if pos == encoded.len() {
                    return (output, Outcome::Truncated);
                }
            }
            DecodeStatus::NeedsOutput => {}
        }
    }
    unreachable!()
}

fuzz_target!(|input: &[u8]| {
    let Some((&control, rest)) = input.split_first() else {
        return;
    };
    let Some((sizes, data)) = rest.split_at_checked(usize::from(control % 8)) else {
        return;
    };

    // Arbitrary input decodes alike in any split, and like the one-shot
    // decoder.
    let whole = push_decode(data, &[]);
    assert!(push_decode(data, sizes) == whole);
    let one_shot = burli::decompress_with_limit(data, 1 << 24);
    match &whole.1 {
        Outcome::Finished { consumed } if *consumed == data.len() => {
            assert!(one_shot == Ok(whole.0.clone()));
        }
        Outcome::Finished { .. } => {
            assert_eq!(one_shot, Err(BurliError::Format("trailing bytes after Brotli stream")));
        }
        Outcome::Truncated => {
            assert_eq!(one_shot, Err(BurliError::Format("unexpected end of Brotli input")));
        }
        Outcome::Failed(error)
            if *error == BurliError::Format("non-zero trailing Brotli padding")
                && one_shot == Err(BurliError::Format("trailing bytes after Brotli stream")) => {}
        Outcome::Failed(error) => assert_eq!(one_shot.as_ref(), Err(error)),
    }

    // Encoded input round-trips in any split.
    let encoded = burli::compress(data, control % 6).unwrap();
    let (decoded, outcome) = push_decode(&encoded, sizes);
    assert_eq!(
        outcome,
        Outcome::Finished {
            consumed: encoded.len()
        }
    );
    assert!(decoded == data);
});
