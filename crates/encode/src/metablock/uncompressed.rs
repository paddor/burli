use alloc::vec::Vec;

use burli_core::{BurliError, CompressError, Options, bits::BitWriter, format::MIN_BLOCK_BITS};

use super::{write_last_empty_meta_block, write_window_bits};

const MAX_META_BLOCK_SIZE: usize = 1 << 24;

pub(crate) fn compress_uncompressed_with_options(
    input: &[u8],
    options: &Options,
) -> Result<Vec<u8>, CompressError> {
    let block_bits = options.block_bits().unwrap_or(MIN_BLOCK_BITS);
    let block_size = 1_usize << block_bits;
    let mut writer = BitWriter::with_capacity(max_uncompressed_size(input.len(), block_size));

    write_window_bits(&mut writer, options.window_bits())?;
    if input.is_empty() {
        write_last_empty_meta_block(&mut writer)?;
        return Ok(writer.into_bytes());
    }

    for chunk in input.chunks(block_size) {
        write_uncompressed_meta_block(&mut writer, chunk)?;
    }
    write_last_empty_meta_block(&mut writer)?;

    Ok(writer.into_bytes())
}

fn max_uncompressed_size(input_len: usize, block_size: usize) -> usize {
    let blocks = input_len.div_ceil(block_size).max(1);
    input_len
        .saturating_add(blocks.saturating_mul(5))
        .saturating_add(2)
}

pub(crate) fn write_uncompressed_meta_block(
    writer: &mut BitWriter,
    input: &[u8],
) -> Result<(), CompressError> {
    if input.is_empty() || input.len() > MAX_META_BLOCK_SIZE {
        return Err(BurliError::Format("invalid uncompressed Brotli block size"));
    }

    write_meta_block_len(writer, input.len())?;
    writer.write_aligned_bytes(input)
}

fn write_meta_block_len(writer: &mut BitWriter, len: usize) -> Result<(), CompressError> {
    let len_minus_one = len - 1;
    let significant_bits = if len == 1 {
        1
    } else {
        usize::BITS - len_minus_one.leading_zeros()
    };
    let nibbles = if significant_bits < 16 {
        4
    } else {
        significant_bits.div_ceil(4) as usize
    };
    debug_assert!((4..=6).contains(&nibbles));

    writer.write_bits(1, 0)?;
    writer.write_bits(2, (nibbles - 4) as u64)?;
    writer.write_bits((nibbles * 4) as u8, len_minus_one as u64)?;
    writer.write_bits(1, 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_empty_stream_with_default_window() {
        assert_eq!(
            compress_uncompressed_with_options(b"", &Options::default().with_quality(0).unwrap())
                .unwrap(),
            [0x3b]
        );
    }

    #[test]
    fn q0_round_trips_through_burli_decoder() {
        let input = b"hello uncompressed brotli";
        let encoded =
            compress_uncompressed_with_options(input, &Options::default().with_quality(0).unwrap())
                .unwrap();

        assert_eq!(burli_decode::decompress(&encoded).unwrap(), input);
    }

    #[test]
    fn q0_splits_blocks_on_block_bits() {
        let options = Options::default()
            .with_quality(0)
            .unwrap()
            .with_window_bits(10)
            .unwrap()
            .with_block_bits(Some(16))
            .unwrap();
        let input = vec![42; (1 << 16) + 3];
        let encoded = compress_uncompressed_with_options(&input, &options).unwrap();

        assert_eq!(burli_decode::decompress(&encoded).unwrap(), input);
    }

    #[test]
    fn q1_to_q5_round_trip_through_encoder_entrypoint() {
        for quality in 1..=5 {
            let encoded = crate::encode::compress_with_options(
                b"uncompressed fallback",
                &Options::default().with_quality(quality).unwrap(),
            )
            .unwrap();

            assert_eq!(
                burli_decode::decompress(&encoded).unwrap(),
                b"uncompressed fallback"
            );
        }
    }

    #[test]
    fn q6_returns_unsupported() {
        assert!(matches!(
            crate::encode::compress_with_options(
                b"hello",
                &Options::default().with_quality(6).unwrap()
            ),
            Err(BurliError::Unsupported(_))
        ));
    }
}
