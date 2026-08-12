use alloc::vec::Vec;

use burli_core::{BurliError, DecompressError, bits::BitReader};

use crate::huffman::PrefixCode;

const LITERAL_ALPHABET_SIZE: usize = 256;
const COMMAND_ALPHABET_SIZE: usize = 704;
const BLOCK_LENGTH_ALPHABET_SIZE: usize = 26;
const LAST_DISTANCES: [usize; 4] = [16, 15, 11, 4];

#[derive(Clone, Debug)]
pub(crate) struct DistanceRing {
    distances: [usize; 4],
}

impl DistanceRing {
    pub(crate) const fn new() -> Self {
        Self {
            distances: LAST_DISTANCES,
        }
    }

    fn resolve(&self, symbol: usize) -> Result<usize, DecompressError> {
        let distance = match symbol {
            0 => self.distances[3],
            1 => self.distances[2],
            2 => self.distances[1],
            3 => self.distances[0],
            4 => self.distances[3].saturating_sub(1),
            5 => self.distances[3] + 1,
            6 => self.distances[3].saturating_sub(2),
            7 => self.distances[3] + 2,
            8 => self.distances[3].saturating_sub(3),
            9 => self.distances[3] + 3,
            10 => self.distances[2].saturating_sub(1),
            11 => self.distances[2] + 1,
            12 => self.distances[2].saturating_sub(2),
            13 => self.distances[2] + 2,
            14 => self.distances[2].saturating_sub(3),
            15 => self.distances[2] + 3,
            _ => return Err(BurliError::Format("invalid Brotli short distance code")),
        };
        if distance == 0 {
            return Err(BurliError::Format("invalid Brotli zero distance"));
        }
        Ok(distance)
    }

    fn push(&mut self, distance: usize) {
        self.distances = [
            self.distances[1],
            self.distances[2],
            self.distances[3],
            distance,
        ];
    }
}

#[derive(Clone, Debug)]
#[cfg(test)]
pub(crate) struct CompressedHeaderProbe {
    literal: usize,
    command: usize,
    distance: usize,
}

#[cfg(test)]
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

#[cfg(test)]
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

pub(crate) fn decode_meta_block(
    reader: &mut BitReader<'_>,
    output: &mut Vec<u8>,
    len: usize,
    max_output_size: usize,
    window_bits: u8,
    distances: &mut DistanceRing,
) -> Result<(), DecompressError> {
    let start = output.len();
    let needed = start.saturating_add(len);
    if needed > max_output_size {
        return Err(BurliError::OutputLimitExceeded {
            limit: max_output_size,
            needed,
        });
    }

    let header = read_header(reader)?;
    if header.literal_block_types != 1
        || header.command_block_types != 1
        || header.distance_block_types != 1
        || header.literal_trees != 1
        || header.distance_trees != 1
    {
        return Err(BurliError::Unsupported(
            "compressed Brotli block switching and context maps not implemented yet",
        ));
    }

    let literal_code = PrefixCode::read(reader, LITERAL_ALPHABET_SIZE)?;
    let command_code = PrefixCode::read(reader, COMMAND_ALPHABET_SIZE)?;
    let distance_code = PrefixCode::read(reader, header.distance_alphabet_size)?;
    let window_size = (1_usize << window_bits) - 16;

    while output.len() < needed {
        let command = read_command(reader, &command_code)?;
        copy_literals(reader, output, needed, command.insert_len, &literal_code)?;
        if output.len() == needed {
            break;
        }
        if output.len() > needed {
            return Err(BurliError::Format("Brotli command exceeds meta-block size"));
        }

        let distance_symbol = if command.reuse_last_distance {
            0
        } else {
            distance_code.decode(reader)? as usize
        };
        let distance = read_distance(
            reader,
            distance_symbol,
            header.npostfix,
            header.ndirect,
            distances,
        )?;
        copy_from_distance(
            output,
            start,
            needed,
            window_size,
            distance,
            command.copy_len,
            distances,
        )?;
    }

    Ok(())
}

#[derive(Clone, Debug)]
struct CompressedHeader {
    literal_block_types: usize,
    command_block_types: usize,
    distance_block_types: usize,
    npostfix: u8,
    ndirect: usize,
    literal_trees: usize,
    distance_trees: usize,
    distance_alphabet_size: usize,
}

fn read_header(reader: &mut BitReader<'_>) -> Result<CompressedHeader, DecompressError> {
    let literal_block_types = read_block_category_header(reader)?;
    let command_block_types = read_block_category_header(reader)?;
    let distance_block_types = read_block_category_header(reader)?;
    let npostfix = reader.read_bits(2)? as u8;
    let ndirect = (reader.read_bits(4)? as usize) << npostfix;

    for _ in 0..literal_block_types {
        let _context_mode = reader.read_bits(2)?;
    }

    let literal_trees = read_var_len_u8(reader)? + 1;
    if literal_trees >= 2 {
        return Err(BurliError::Unsupported(
            "Brotli literal context maps not implemented yet",
        ));
    }

    let distance_trees = read_var_len_u8(reader)? + 1;
    if distance_trees >= 2 {
        return Err(BurliError::Unsupported(
            "Brotli distance context maps not implemented yet",
        ));
    }

    let distance_alphabet_size = 16 + ndirect + (48 << npostfix);
    Ok(CompressedHeader {
        literal_block_types,
        command_block_types,
        distance_block_types,
        npostfix,
        ndirect,
        literal_trees,
        distance_trees,
        distance_alphabet_size,
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

#[derive(Clone, Copy, Debug)]
struct Command {
    insert_len: usize,
    copy_len: usize,
    reuse_last_distance: bool,
}

fn read_command(
    reader: &mut BitReader<'_>,
    command_code: &PrefixCode,
) -> Result<Command, DecompressError> {
    let code = command_code.decode(reader)? as usize;
    let (insert_code, copy_code, reuse_last_distance) = command_code_parts(code)?;
    let (insert_base, insert_extra_bits) = insert_length_prefix(insert_code)?;
    let (copy_base, copy_extra_bits) = copy_length_prefix(copy_code)?;
    let insert_len = insert_base + reader.read_bits(insert_extra_bits)? as usize;
    let copy_len = copy_base + reader.read_bits(copy_extra_bits)? as usize;

    Ok(Command {
        insert_len,
        copy_len,
        reuse_last_distance,
    })
}

fn command_code_parts(code: usize) -> Result<(usize, usize, bool), DecompressError> {
    if code >= COMMAND_ALPHABET_SIZE {
        return Err(BurliError::Format("invalid Brotli command code"));
    }

    let low = code & 0b111;
    let high = (code >> 3) & 0b111;
    let cell = code >> 6;
    let reuse_last_distance = code < 128;
    let (insert_base, copy_base) = match cell {
        0 | 2 => (0, 0),
        1 | 3 => (0, 8),
        4 => (8, 0),
        5 => (8, 8),
        6 => (0, 16),
        7 => (16, 0),
        8 => (8, 16),
        9 => (16, 8),
        10 => (16, 16),
        _ => return Err(BurliError::Format("invalid Brotli command code")),
    };

    Ok((insert_base + high, copy_base + low, reuse_last_distance))
}

fn insert_length_prefix(code: usize) -> Result<(usize, u8), DecompressError> {
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

fn copy_length_prefix(code: usize) -> Result<(usize, u8), DecompressError> {
    match code {
        0..=7 => Ok((code + 2, 0)),
        8..=9 => Ok((10 + (code - 8) * 2, 1)),
        10..=11 => Ok((14 + (code - 10) * 4, 2)),
        12..=13 => Ok((22 + (code - 12) * 8, 3)),
        14..=15 => Ok((38 + (code - 14) * 16, 4)),
        16 => Ok((70, 5)),
        17 => Ok((102, 5)),
        18 => Ok((134, 6)),
        19 => Ok((198, 7)),
        20 => Ok((326, 8)),
        21 => Ok((582, 9)),
        22 => Ok((1094, 10)),
        23 => Ok((2118, 24)),
        _ => Err(BurliError::Format("invalid Brotli copy length code")),
    }
}

fn copy_literals(
    reader: &mut BitReader<'_>,
    output: &mut Vec<u8>,
    needed: usize,
    count: usize,
    literal_code: &PrefixCode,
) -> Result<(), DecompressError> {
    let end = output
        .len()
        .checked_add(count)
        .ok_or(BurliError::Format("Brotli literal run length overflow"))?;
    if end > needed {
        return Err(BurliError::Format(
            "Brotli literal run exceeds meta-block size",
        ));
    }

    for _ in 0..count {
        let literal = literal_code.decode(reader)?;
        if literal > u16::from(u8::MAX) {
            return Err(BurliError::Format("invalid Brotli literal symbol"));
        }
        output.push(literal as u8);
    }
    Ok(())
}

fn read_distance(
    reader: &mut BitReader<'_>,
    symbol: usize,
    npostfix: u8,
    ndirect: usize,
    distances: &DistanceRing,
) -> Result<usize, DecompressError> {
    if symbol < 16 {
        return distances.resolve(symbol);
    }
    if symbol < 16 + ndirect {
        return Ok(symbol - 15);
    }

    let adjusted = symbol - ndirect - 16;
    let ndistbits = 1 + (adjusted >> (npostfix + 1));
    if ndistbits > 24 {
        return Err(BurliError::Format("invalid Brotli distance extra bits"));
    }
    let dextra = reader.read_bits(ndistbits as u8)? as usize;
    let hcode = adjusted >> npostfix;
    let lcode = adjusted & ((1_usize << npostfix) - 1);
    let offset = ((2 + (hcode & 1)) << ndistbits) - 4;
    Ok(((offset + dextra) << npostfix) + lcode + ndirect + 1)
}

fn copy_from_distance(
    output: &mut Vec<u8>,
    meta_block_start: usize,
    needed: usize,
    window_size: usize,
    distance: usize,
    copy_len: usize,
    distances: &mut DistanceRing,
) -> Result<(), DecompressError> {
    let produced = output.len();
    let max_allowed_distance = window_size.min(produced);
    if distance > max_allowed_distance {
        return Err(BurliError::Unsupported(
            "Brotli static dictionary references not implemented yet",
        ));
    }
    if distance == 0 || distance > produced {
        return Err(BurliError::Format("invalid Brotli backward distance"));
    }

    let end = produced
        .checked_add(copy_len)
        .ok_or(BurliError::Format("Brotli copy length overflow"))?;
    if end > needed {
        return Err(BurliError::Format("Brotli copy exceeds meta-block size"));
    }
    if end < meta_block_start {
        return Err(BurliError::Format("Brotli copy output position underflow"));
    }

    for _ in 0..copy_len {
        let src = output.len() - distance;
        let byte = output[src];
        output.push(byte);
    }
    distances.push(distance);
    Ok(())
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
