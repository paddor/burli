use alloc::vec::Vec;

use burli_core::{
    BurliError, CompressError, Options,
    bits::BitWriter,
    format::{MAX_WINDOW_BITS, MIN_BLOCK_BITS, MIN_WINDOW_BITS},
};

const MAX_META_BLOCK_SIZE: usize = 1 << 24;
const MAX_STORED_FALLBACK_QUALITY: u8 = 5;

pub fn compress_with_options(input: &[u8], options: &Options) -> Result<Vec<u8>, CompressError> {
    if options.quality_value() > MAX_STORED_FALLBACK_QUALITY {
        return Err(BurliError::Unsupported(
            "only q0..q5 stored Brotli encoding is implemented yet",
        ));
    }

    let block_bits = options.block_bits_value().unwrap_or(MIN_BLOCK_BITS);
    let block_size = 1_usize << block_bits;
    let mut writer = BitWriter::with_capacity(max_stored_size(input.len(), block_size));

    write_window_bits(&mut writer, options.window_bits_value())?;
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

fn max_stored_size(input_len: usize, block_size: usize) -> usize {
    let blocks = input_len.div_ceil(block_size).max(1);
    input_len
        .saturating_add(blocks.saturating_mul(5))
        .saturating_add(2)
}

fn write_window_bits(writer: &mut BitWriter, window_bits: u8) -> Result<(), CompressError> {
    if !(MIN_WINDOW_BITS..=MAX_WINDOW_BITS).contains(&window_bits) {
        return Err(BurliError::InvalidWindowBits(window_bits));
    }

    if window_bits == 16 {
        writer.write_bits(1, 0)
    } else if window_bits == 17 {
        writer.write_bits(7, 1)
    } else if window_bits > 17 {
        let bits = ((window_bits - 17) << 1) | 1;
        writer.write_bits(4, u64::from(bits))
    } else {
        let bits = ((window_bits - 8) << 4) | 1;
        writer.write_bits(7, u64::from(bits))
    }
}

fn write_uncompressed_meta_block(
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

fn write_last_empty_meta_block(writer: &mut BitWriter) -> Result<(), CompressError> {
    writer.write_bits(1, 1)?;
    writer.write_bits(1, 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_empty_stream_with_default_window() {
        assert_eq!(
            compress_with_options(b"", &Options::default().quality(0).unwrap()).unwrap(),
            [0x3b]
        );
    }

    #[test]
    fn q0_round_trips_through_burli_decoder() {
        let input = b"hello stored brotli";
        let encoded =
            compress_with_options(input, &Options::default().quality(0).unwrap()).unwrap();

        assert_eq!(burli_decode::decompress(&encoded).unwrap(), input);
    }

    #[test]
    fn q0_splits_blocks_on_block_bits() {
        let options = Options::default()
            .quality(0)
            .unwrap()
            .window_bits(10)
            .unwrap()
            .block_bits(Some(16))
            .unwrap();
        let input = vec![42; (1 << 16) + 3];
        let encoded = compress_with_options(&input, &options).unwrap();

        assert_eq!(burli_decode::decompress(&encoded).unwrap(), input);
    }

    #[test]
    fn q1_to_q5_use_stored_fallback() {
        for quality in 1..=5 {
            let encoded = compress_with_options(
                b"stored fallback",
                &Options::default().quality(quality).unwrap(),
            )
            .unwrap();

            assert_eq!(
                burli_decode::decompress(&encoded).unwrap(),
                b"stored fallback"
            );
        }
    }

    #[test]
    fn q6_returns_unsupported() {
        assert!(matches!(
            compress_with_options(b"hello", &Options::default().quality(6).unwrap()),
            Err(BurliError::Unsupported(_))
        ));
    }
}
