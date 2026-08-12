use alloc::vec::Vec;

use burli_core::{
    BurliError, CompressError, Options,
    bits::BitWriter,
    format::{MAX_WINDOW_BITS, MIN_WINDOW_BITS},
};

const MAX_LITERAL_ONLY_QUALITY: u8 = 5;
const MAX_META_BLOCK_SIZE: usize = 1 << 24;
const CODE_LENGTH_ORDER: [u8; 18] = [1, 2, 3, 4, 0, 5, 17, 6, 16, 7, 8, 9, 10, 11, 12, 13, 14, 15];

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

    for chunk in input.chunks(MAX_META_BLOCK_SIZE) {
        write_compressed_literal_meta_block(&mut writer, chunk)?;
    }
    write_last_empty_meta_block(&mut writer)?;

    Ok(writer.into_bytes())
}

fn max_literal_only_size(input_len: usize) -> usize {
    input_len
        .saturating_add(input_len / 1024)
        .saturating_add(64)
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
    input: &[u8],
) -> Result<(), CompressError> {
    if input.is_empty() || input.len() > MAX_META_BLOCK_SIZE {
        return Err(BurliError::Format("invalid compressed Brotli block size"));
    }

    let insert = insert_length_code(input.len())?;
    let command_symbol = command_symbol_for_insert(insert.code)?;

    write_meta_block_len(writer, input.len())?;
    write_var_len_u8(writer, 0)?;
    write_var_len_u8(writer, 0)?;
    write_var_len_u8(writer, 0)?;
    writer.write_bits(2, 0)?;
    writer.write_bits(4, 0)?;
    writer.write_bits(2, 0)?;
    write_var_len_u8(writer, 0)?;
    write_var_len_u8(writer, 0)?;
    write_fixed_literal_prefix_code(writer)?;
    write_simple_prefix_code_single(writer, 704, command_symbol)?;
    write_simple_prefix_code_single(writer, 64, 0)?;
    writer.write_bits(insert.extra_bits, insert.extra)?;
    for &literal in input {
        writer.write_bits(8, u64::from(reverse_bits(literal, 8)))?;
    }
    Ok(())
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

fn write_fixed_literal_prefix_code(writer: &mut BitWriter) -> Result<(), CompressError> {
    writer.write_bits(2, 0)?;
    for symbol in CODE_LENGTH_ORDER {
        let len = if symbol == 8 { 1 } else { 0 };
        write_code_length_code_len(writer, len)?;
    }
    Ok(())
}

fn write_code_length_code_len(writer: &mut BitWriter, len: u8) -> Result<(), CompressError> {
    match len {
        0 => writer.write_bits(2, 0),
        1 => writer.write_bits(4, 7),
        _ => Err(BurliError::Format("unsupported Brotli code length code")),
    }
}

#[derive(Clone, Copy, Debug)]
struct InsertLengthCode {
    code: usize,
    extra_bits: u8,
    extra: u64,
}

fn insert_length_code(len: usize) -> Result<InsertLengthCode, CompressError> {
    for code in 0..=23 {
        let (base, extra_bits) = insert_length_prefix(code)?;
        let span = 1_usize << extra_bits;
        if (base..base + span).contains(&len) {
            return Ok(InsertLengthCode {
                code,
                extra_bits,
                extra: (len - base) as u64,
            });
        }
    }
    Err(BurliError::Format("Brotli insert length exceeds range"))
}

fn insert_length_prefix(code: usize) -> Result<(usize, u8), CompressError> {
    match code {
        0..=5 => Ok((code, 0)),
        6..=7 => Ok((6 + (code - 6) * 2, 1)),
        8..=9 => Ok((10 + (code - 8) * 4, 2)),
        10..=11 => Ok((18 + (code - 10) * 8, 3)),
        12..=13 => Ok((34 + (code - 12) * 16, 4)),
        14..=15 => Ok((66 + (code - 14) * 32, 5)),
        16 => Ok((130, 6)),
        17 => Ok((194, 7)),
        18 => Ok((322, 8)),
        19 => Ok((578, 9)),
        20 => Ok((1090, 10)),
        21 => Ok((2114, 12)),
        22 => Ok((6210, 14)),
        23 => Ok((22594, 24)),
        _ => Err(BurliError::Format("invalid Brotli insert length code")),
    }
}

fn command_symbol_for_insert(insert_code: usize) -> Result<u16, CompressError> {
    let symbol = match insert_code {
        0..=7 => insert_code * 8,
        8..=15 => 256 + (insert_code - 8) * 8,
        16..=23 => 448 + (insert_code - 16) * 8,
        _ => return Err(BurliError::Format("invalid Brotli insert length code")),
    };
    Ok(symbol as u16)
}

fn reverse_bits(value: u8, width: u8) -> u8 {
    let mut reversed = 0;
    for bit in 0..width {
        reversed <<= 1;
        reversed |= (value >> bit) & 1;
    }
    reversed
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

    #[test]
    fn q1_round_trips_mixed_literals() {
        let input = b"function demo(){return 42;}";
        let encoded =
            compress_with_options(input, &Options::default().quality(1).unwrap()).unwrap();

        assert_eq!(burli_decode::decompress(&encoded).unwrap(), input);
    }

    #[test]
    fn q5_round_trips_long_literal_run() {
        let input = vec![b'a'; 3000];
        let encoded =
            compress_with_options(&input, &Options::default().quality(5).unwrap()).unwrap();

        assert_eq!(burli_decode::decompress(&encoded).unwrap(), input);
    }
}
