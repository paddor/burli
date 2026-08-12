use alloc::vec::Vec;

use burli_core::{
    BurliError, CompressError, Options,
    bits::BitWriter,
    format::{MAX_WINDOW_BITS, MIN_WINDOW_BITS},
};

const MAX_LITERAL_ONLY_QUALITY: u8 = 5;
const COMMAND_INSERT_ONE_COPY_TWO: u16 = 8;

pub fn compress_with_options(input: &[u8], options: &Options) -> Result<Vec<u8>, CompressError> {
    if options.quality_value() > MAX_LITERAL_ONLY_QUALITY {
        return Err(BurliError::Unsupported(
            "only q0..q5 Brotli encoding is implemented yet",
        ));
    }

    let mut writer = BitWriter::with_capacity(max_literal_only_size(input.len()));
    write_window_bits(&mut writer, options.window_bits_value())?;
    if input.is_empty() {
        write_last_empty_meta_block(&mut writer)?;
        return Ok(writer.into_bytes());
    }

    for &byte in input {
        write_compressed_literal_meta_block(&mut writer, byte)?;
    }
    write_last_empty_meta_block(&mut writer)?;

    Ok(writer.into_bytes())
}

fn max_literal_only_size(input_len: usize) -> usize {
    input_len.saturating_mul(12).saturating_add(2)
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

fn write_compressed_literal_meta_block(
    writer: &mut BitWriter,
    literal: u8,
) -> Result<(), CompressError> {
    write_meta_block_len(writer, 1)?;
    write_var_len_u8(writer, 0)?;
    write_var_len_u8(writer, 0)?;
    write_var_len_u8(writer, 0)?;
    writer.write_bits(2, 0)?;
    writer.write_bits(4, 0)?;
    writer.write_bits(2, 0)?;
    write_var_len_u8(writer, 0)?;
    write_var_len_u8(writer, 0)?;
    write_simple_prefix_code_single(writer, 256, u16::from(literal))?;
    write_simple_prefix_code_single(writer, 704, COMMAND_INSERT_ONE_COPY_TWO)?;
    write_simple_prefix_code_single(writer, 64, 0)
}

fn write_meta_block_len(writer: &mut BitWriter, len: usize) -> Result<(), CompressError> {
    let len_minus_one = len - 1;
    writer.write_bits(1, 0)?;
    writer.write_bits(2, 0)?;
    writer.write_bits(16, len_minus_one as u64)?;
    writer.write_bits(1, 0)
}

fn write_var_len_u8(writer: &mut BitWriter, value: usize) -> Result<(), CompressError> {
    if value == 0 {
        return writer.write_bits(1, 0);
    }
    if value == 1 {
        writer.write_bits(1, 1)?;
        return writer.write_bits(3, 0);
    }

    let width = usize::BITS - (value - 1).leading_zeros();
    if width > 8 {
        return Err(BurliError::Format("Brotli varlen u8 value exceeds range"));
    }
    writer.write_bits(1, 1)?;
    writer.write_bits(3, u64::from(width))?;
    writer.write_bits(width as u8, (value - (1_usize << width)) as u64)
}

fn write_simple_prefix_code_single(
    writer: &mut BitWriter,
    alphabet_size: usize,
    symbol: u16,
) -> Result<(), CompressError> {
    let alphabet_bits = alphabet_bits(alphabet_size);
    if usize::from(symbol) >= alphabet_size {
        return Err(BurliError::Format("Brotli prefix symbol exceeds alphabet"));
    }

    writer.write_bits(2, 1)?;
    writer.write_bits(2, 0)?;
    writer.write_bits(alphabet_bits, u64::from(symbol))
}

fn alphabet_bits(alphabet_size: usize) -> u8 {
    let value = alphabet_size.saturating_sub(1);
    (usize::BITS - value.leading_zeros()) as u8
}

fn write_last_empty_meta_block(writer: &mut BitWriter) -> Result<(), CompressError> {
    writer.write_bits(1, 1)?;
    writer.write_bits(1, 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn q1_emits_compressed_stream() {
        let encoded = compress_with_options(b"x", &Options::default().quality(1).unwrap()).unwrap();

        assert_ne!(
            encoded,
            crate::stored::compress_with_options(b"x", &Options::default().quality(0).unwrap())
                .unwrap()
        );
        assert_eq!(burli_decode::decompress(&encoded).unwrap(), b"x");
    }
}
