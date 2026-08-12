use burli_core::{BurliError, DecompressError, bits::BitReader};

use crate::huffman::PrefixCode;

const BLOCK_LENGTH_ALPHABET_SIZE: usize = 26;

#[derive(Clone, Debug)]
pub(crate) struct CompressedHeaderProbe {
    literal: usize,
    command: usize,
    distance: usize,
}

impl CompressedHeaderProbe {
    pub(crate) const fn literal_block_types(&self) -> usize {
        self.literal
    }

    pub(crate) const fn command_block_types(&self) -> usize {
        self.command
    }

    pub(crate) const fn distance_block_types(&self) -> usize {
        self.distance
    }
}

pub(crate) fn read_header_probe(
    reader: &mut BitReader<'_>,
) -> Result<CompressedHeaderProbe, DecompressError> {
    let literal_block_types = read_block_category_header(reader)?;
    let command_block_types = read_block_category_header(reader)?;
    let distance_block_types = read_block_category_header(reader)?;

    Ok(CompressedHeaderProbe {
        literal: literal_block_types,
        command: command_block_types,
        distance: distance_block_types,
    })
}

fn read_block_category_header(reader: &mut BitReader<'_>) -> Result<usize, DecompressError> {
    let block_types = read_var_len_u8(reader)? + 1;
    if block_types >= 2 {
        let _block_type_code = PrefixCode::read(reader, block_types + 2)?;
        let block_count_code = PrefixCode::read(reader, BLOCK_LENGTH_ALPHABET_SIZE)?;
        let _first_block_count = read_block_count(reader, &block_count_code)?;
    }
    Ok(block_types)
}

fn read_var_len_u8(reader: &mut BitReader<'_>) -> Result<usize, DecompressError> {
    if !reader.read_bit()? {
        return Ok(0);
    }

    let width = reader.read_bits(3)? as u8;
    if width == 0 {
        return Ok(1);
    }

    let extra = reader.read_bits(width)?;
    Ok((1_usize << width) + extra as usize)
}

fn read_block_count(
    reader: &mut BitReader<'_>,
    code: &PrefixCode,
) -> Result<usize, DecompressError> {
    let symbol = code.decode(reader)? as usize;
    let (base, extra_bits) = block_count_prefix(symbol)?;
    Ok(base + reader.read_bits(extra_bits)? as usize)
}

fn block_count_prefix(symbol: usize) -> Result<(usize, u8), DecompressError> {
    match symbol {
        0 => Ok((1, 2)),
        1 => Ok((5, 2)),
        2 => Ok((9, 2)),
        3 => Ok((13, 2)),
        4 => Ok((17, 3)),
        5 => Ok((25, 3)),
        6 => Ok((33, 3)),
        7 => Ok((41, 3)),
        8 => Ok((49, 4)),
        9 => Ok((65, 4)),
        10 => Ok((81, 4)),
        11 => Ok((97, 4)),
        12 => Ok((113, 5)),
        13 => Ok((145, 5)),
        14 => Ok((177, 5)),
        15 => Ok((209, 5)),
        16 => Ok((241, 6)),
        17 => Ok((305, 6)),
        18 => Ok((369, 7)),
        19 => Ok((497, 8)),
        20 => Ok((753, 9)),
        21 => Ok((1265, 10)),
        22 => Ok((2289, 11)),
        23 => Ok((4337, 12)),
        24 => Ok((8433, 13)),
        25 => Ok((16625, 24)),
        _ => Err(BurliError::Format("invalid Brotli block count code")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use burli_core::bits::BitWriter;

    #[test]
    fn reads_single_type_headers_without_prefix_codes() {
        let mut bits = BitWriter::new();
        bits.write_bits(1, 0).unwrap();
        bits.write_bits(1, 0).unwrap();
        bits.write_bits(1, 0).unwrap();
        let bytes = bits.into_bytes();
        let mut reader = BitReader::new(&bytes);
        let probe = read_header_probe(&mut reader).unwrap();

        assert_eq!(probe.literal_block_types(), 1);
        assert_eq!(probe.command_block_types(), 1);
        assert_eq!(probe.distance_block_types(), 1);
    }

    #[test]
    fn reads_var_len_u8_boundaries() {
        let mut bits = BitWriter::new();
        bits.write_bits(1, 0).unwrap();
        bits.write_bits(1, 1).unwrap();
        bits.write_bits(3, 0).unwrap();
        bits.write_bits(1, 1).unwrap();
        bits.write_bits(3, 3).unwrap();
        bits.write_bits(3, 5).unwrap();
        let bytes = bits.into_bytes();
        let mut reader = BitReader::new(&bytes);

        assert_eq!(read_var_len_u8(&mut reader).unwrap(), 0);
        assert_eq!(read_var_len_u8(&mut reader).unwrap(), 1);
        assert_eq!(read_var_len_u8(&mut reader).unwrap(), 13);
    }
}
