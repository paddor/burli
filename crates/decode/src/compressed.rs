use alloc::vec;
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

pub(crate) fn decode_meta_block(
    reader: &mut BitReader<'_>,
    output: &mut Vec<u8>,
    len: usize,
    max_output_size: usize,
    window_bits: u8,
    distances: &mut DistanceRing,
) -> Result<(), DecompressError> {
    decode_meta_block_with_base(
        reader,
        output,
        0,
        len,
        max_output_size,
        window_bits,
        distances,
    )
}

pub(crate) fn decode_meta_block_with_base(
    reader: &mut BitReader<'_>,
    output: &mut Vec<u8>,
    output_base: usize,
    len: usize,
    max_output_size: usize,
    window_bits: u8,
    distances: &mut DistanceRing,
) -> Result<(), DecompressError> {
    let start = output.len();
    let needed = start
        .checked_add(len)
        .ok_or(BurliError::Format("Brotli output length overflow"))?;
    let global_needed = output_base
        .checked_add(needed)
        .ok_or(BurliError::Format("Brotli output length overflow"))?;
    if global_needed > max_output_size {
        return Err(BurliError::OutputLimitExceeded {
            limit: max_output_size,
            needed: global_needed,
        });
    }
    output.reserve(needed - output.len());

    let mut header = read_header(reader)?;

    let literal_codes =
        read_prefix_codes(reader, header.literal_tree_count(), LITERAL_ALPHABET_SIZE)?;
    let command_codes = read_prefix_codes(reader, header.commands.types(), COMMAND_ALPHABET_SIZE)?;
    let distance_codes = read_prefix_codes(
        reader,
        header.distance_tree_count(),
        header.distance_alphabet_size,
    )?;
    let window_size = (1_usize << window_bits) - 16;

    while output.len() < needed {
        let command_block_type = header.commands.current_type(reader)?;
        let command = read_command(reader, &command_codes[command_block_type])?;
        header.commands.consume_one();
        copy_literals(
            reader,
            output,
            needed,
            command.insert_len,
            &literal_codes,
            &mut header,
        )?;
        if output.len() == needed {
            break;
        }
        if output.len() > needed {
            return Err(BurliError::Format("Brotli command exceeds meta-block size"));
        }

        let distance_symbol = if command.reuse_last_distance {
            0
        } else {
            let distance_block_type = header.distances.current_type(reader)?;
            let tree_index =
                header.distance_context_map[distance_block_type * 4 + command.distance_context];
            header.distances.consume_one();
            distance_codes[tree_index].decode(reader)? as usize
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
            CopyRequest {
                meta_block_start: start,
                needed,
                window_size,
                output_base,
                distance,
                len: command.copy_len,
                push_distance: distance_symbol != 0,
            },
            distances,
        )?;
    }

    Ok(())
}

#[derive(Clone, Copy, Debug)]
struct CopyRequest {
    meta_block_start: usize,
    needed: usize,
    window_size: usize,
    output_base: usize,
    distance: usize,
    len: usize,
    push_distance: bool,
}

fn copy_from_distance(
    output: &mut Vec<u8>,
    request: CopyRequest,
    distances: &mut DistanceRing,
) -> Result<(), DecompressError> {
    let produced = output.len();
    let global_produced = request
        .output_base
        .checked_add(produced)
        .ok_or(BurliError::Format("Brotli output length overflow"))?;
    let max_allowed_distance = request.window_size.min(global_produced);
    if request.distance > max_allowed_distance {
        let word = crate::dictionary::lookup(request.distance, max_allowed_distance, request.len)?;
        let end = produced
            .checked_add(word.len())
            .ok_or(BurliError::Format("Brotli dictionary copy length overflow"))?;
        if end > request.needed {
            return Err(BurliError::Format(
                "Brotli dictionary copy exceeds meta-block size",
            ));
        }
        output.extend_from_slice(&word);
        return Ok(());
    }
    if request.distance == 0 || request.distance > produced {
        return Err(BurliError::Format("invalid Brotli backward distance"));
    }

    checked_backward_copy_end(produced, request)?;

    for _ in 0..request.len {
        let src = output.len() - request.distance;
        let byte = output[src];
        output.push(byte);
    }
    if request.push_distance {
        distances.push(request.distance);
    }
    Ok(())
}

fn checked_backward_copy_end(
    produced: usize,
    request: CopyRequest,
) -> Result<usize, DecompressError> {
    let end = produced
        .checked_add(request.len)
        .ok_or(BurliError::Format("Brotli copy length overflow"))?;
    if end > request.needed {
        return Err(BurliError::Format("Brotli copy exceeds meta-block size"));
    }
    if end < request.meta_block_start {
        return Err(BurliError::Format("Brotli copy output position underflow"));
    }
    Ok(end)
}

#[derive(Clone, Debug)]
struct CompressedHeader {
    literals: BlockCategory,
    commands: BlockCategory,
    distances: BlockCategory,
    npostfix: u8,
    ndirect: usize,
    context_modes: Vec<u8>,
    literal_context_map: Vec<usize>,
    distance_context_map: Vec<usize>,
    distance_alphabet_size: usize,
}

#[derive(Clone, Debug)]
struct BlockCategory {
    block_types: usize,
    current_type: usize,
    previous_type: usize,
    remaining: usize,
    type_code: Option<PrefixCode>,
    count_code: Option<PrefixCode>,
}

impl BlockCategory {
    const fn single() -> Self {
        Self {
            block_types: 1,
            current_type: 0,
            previous_type: 1,
            remaining: usize::MAX,
            type_code: None,
            count_code: None,
        }
    }

    fn new(
        block_types: usize,
        type_code: PrefixCode,
        count_code: PrefixCode,
        remaining: usize,
    ) -> Self {
        Self {
            block_types,
            current_type: 0,
            previous_type: 1,
            remaining,
            type_code: Some(type_code),
            count_code: Some(count_code),
        }
    }

    const fn types(&self) -> usize {
        self.block_types
    }

    fn current_type(&mut self, reader: &mut BitReader<'_>) -> Result<usize, DecompressError> {
        if self.remaining == 0 {
            self.switch(reader)?;
        }
        Ok(self.current_type)
    }

    fn consume_one(&mut self) {
        self.remaining = self.remaining.saturating_sub(1);
    }

    fn switch(&mut self, reader: &mut BitReader<'_>) -> Result<(), DecompressError> {
        let type_code = self
            .type_code
            .as_ref()
            .ok_or(BurliError::Format("missing Brotli block type code"))?;
        let count_code = self
            .count_code
            .as_ref()
            .ok_or(BurliError::Format("missing Brotli block count code"))?;
        let symbol = type_code.decode(reader)? as usize;
        let next_type = match symbol {
            0 => self.previous_type,
            1 => (self.current_type + 1) % self.block_types,
            _ => {
                let value = symbol - 2;
                if value >= self.block_types {
                    return Err(BurliError::Format("invalid Brotli block type"));
                }
                value
            }
        };
        self.previous_type = self.current_type;
        self.current_type = next_type;
        self.remaining = read_block_count(reader, count_code)?;
        Ok(())
    }
}

impl CompressedHeader {
    fn literal_tree_count(&self) -> usize {
        self.literal_context_map.iter().copied().max().unwrap_or(0) + 1
    }

    fn distance_tree_count(&self) -> usize {
        self.distance_context_map.iter().copied().max().unwrap_or(0) + 1
    }
}

fn read_header(reader: &mut BitReader<'_>) -> Result<CompressedHeader, DecompressError> {
    let literals = read_block_category_header(reader)?;
    let commands = read_block_category_header(reader)?;
    let distances = read_block_category_header(reader)?;
    let npostfix = reader.read_bits(2)? as u8;
    let ndirect = (reader.read_bits(4)? as usize) << npostfix;

    let mut context_modes = Vec::with_capacity(literals.types());
    for _ in 0..literals.types() {
        context_modes.push(reader.read_bits(2)? as u8);
    }

    let literal_trees = read_var_len_u8(reader)? + 1;
    let literal_context_map = read_context_map(reader, literals.types() * 64, literal_trees)?;

    let distance_trees = read_var_len_u8(reader)? + 1;
    let distance_context_map = read_context_map(reader, distances.types() * 4, distance_trees)?;

    let distance_alphabet_size = 16 + ndirect + (48 << npostfix);
    Ok(CompressedHeader {
        literals,
        commands,
        distances,
        npostfix,
        ndirect,
        context_modes,
        literal_context_map,
        distance_context_map,
        distance_alphabet_size,
    })
}

fn read_block_category_header(
    reader: &mut BitReader<'_>,
) -> Result<BlockCategory, DecompressError> {
    let block_types = read_var_len_u8(reader)? + 1;
    if block_types == 1 {
        return Ok(BlockCategory::single());
    }

    let block_type_code = PrefixCode::read(reader, block_types + 2)?;
    let block_count_code = PrefixCode::read(reader, BLOCK_LENGTH_ALPHABET_SIZE)?;
    let first_block_count = read_block_count(reader, &block_count_code)?;
    Ok(BlockCategory::new(
        block_types,
        block_type_code,
        block_count_code,
        first_block_count,
    ))
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

fn read_prefix_codes(
    reader: &mut BitReader<'_>,
    count: usize,
    alphabet_size: usize,
) -> Result<Vec<PrefixCode>, DecompressError> {
    let mut codes = Vec::with_capacity(count);
    for _ in 0..count {
        codes.push(PrefixCode::read(reader, alphabet_size)?);
    }
    Ok(codes)
}

fn read_context_map(
    reader: &mut BitReader<'_>,
    size: usize,
    tree_count: usize,
) -> Result<Vec<usize>, DecompressError> {
    if tree_count == 0 || tree_count > 256 {
        return Err(BurliError::Format("invalid Brotli context tree count"));
    }
    if tree_count == 1 {
        return Ok(vec![0; size]);
    }

    let rlemax = read_context_rlemax(reader)?;
    let code = PrefixCode::read(reader, tree_count + rlemax)?;
    let mut map = Vec::with_capacity(size);
    while map.len() < size {
        let symbol = code.decode(reader)? as usize;
        if rlemax != 0 && (1..=rlemax).contains(&symbol) {
            let repeat = (1_usize << symbol) + reader.read_bits(symbol as u8)? as usize;
            let end = map
                .len()
                .checked_add(repeat)
                .ok_or(BurliError::Format("Brotli context map repeat overflow"))?;
            if end > size {
                return Err(BurliError::Format("Brotli context map repeat exceeds size"));
            }
            map.resize(end, 0);
        } else {
            let value = if rlemax == 0 {
                symbol
            } else {
                symbol.saturating_sub(rlemax)
            };
            if value >= tree_count {
                return Err(BurliError::Format("invalid Brotli context map value"));
            }
            map.push(value);
        }
    }

    if reader.read_bit()? {
        inverse_move_to_front(&mut map)?;
    }
    Ok(map)
}

fn read_context_rlemax(reader: &mut BitReader<'_>) -> Result<usize, DecompressError> {
    if !reader.read_bit()? {
        return Ok(0);
    }
    Ok(reader.read_bits(4)? as usize + 1)
}

fn inverse_move_to_front(map: &mut [usize]) -> Result<(), DecompressError> {
    let mut mtf = [0_usize; 256];
    for (index, slot) in mtf.iter_mut().enumerate() {
        *slot = index;
    }

    for value in map {
        let index = *value;
        let Some(&resolved) = mtf.get(index) else {
            return Err(BurliError::Format("invalid Brotli move-to-front index"));
        };
        for shift in (1..=index).rev() {
            mtf[shift] = mtf[shift - 1];
        }
        mtf[0] = resolved;
        *value = resolved;
    }
    Ok(())
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
    distance_context: usize,
}

fn read_command(
    reader: &mut BitReader<'_>,
    command_code: &PrefixCode,
) -> Result<Command, DecompressError> {
    let code = command_code.decode(reader)? as usize;
    let (insert_code, copy_code, reuse_last_distance, distance_context) = command_code_parts(code)?;
    let (insert_base, insert_extra_bits) = insert_length_prefix(insert_code)?;
    let (copy_base, copy_extra_bits) = copy_length_prefix(copy_code)?;
    let insert_len = insert_base + reader.read_bits(insert_extra_bits)? as usize;
    let copy_len = copy_base + reader.read_bits(copy_extra_bits)? as usize;

    Ok(Command {
        insert_len,
        copy_len,
        reuse_last_distance,
        distance_context,
    })
}

fn command_code_parts(code: usize) -> Result<(usize, usize, bool, usize), DecompressError> {
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

    let distance_context = if matches!(cell, 0 | 2 | 4 | 7) && low <= 2 {
        low
    } else {
        3
    };

    Ok((
        insert_base + high,
        copy_base + low,
        reuse_last_distance,
        distance_context,
    ))
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
    literal_codes: &[PrefixCode],
    header: &mut CompressedHeader,
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
        let literal_block_type = header.literals.current_type(reader)?;
        let context = literal_context(output, header, literal_block_type)?;
        let tree_index = header.literal_context_map[literal_block_type * 64 + context];
        let literal = literal_codes[tree_index].decode(reader)?;
        header.literals.consume_one();
        if literal > u16::from(u8::MAX) {
            return Err(BurliError::Format("invalid Brotli literal symbol"));
        }
        output.push(literal as u8);
    }
    Ok(())
}

fn literal_context(
    output: &[u8],
    header: &CompressedHeader,
    block_type: usize,
) -> Result<usize, DecompressError> {
    literal_context_for_mode(output, header.context_modes[block_type])
}

fn literal_context_for_mode(output: &[u8], mode: u8) -> Result<usize, DecompressError> {
    let p1 = output.last().copied().unwrap_or(0);
    let p2 = output
        .len()
        .checked_sub(2)
        .and_then(|index| output.get(index))
        .copied()
        .unwrap_or(0);
    let context = match mode {
        0 => p1 & 0x3f,
        1 => p1 >> 2,
        2 => {
            crate::context_lookup::kContextLookup[2][usize::from(p1)]
                | crate::context_lookup::kContextLookup[2][usize::from(p2) + 256]
        }
        3 => {
            crate::context_lookup::kContextLookup[3][usize::from(p1)]
                | crate::context_lookup::kContextLookup[3][usize::from(p2) + 256]
        }
        _ => return Err(BurliError::Format("invalid Brotli literal context mode")),
    };
    Ok(usize::from(context))
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
        let literal = read_block_category_header(&mut reader).unwrap();
        let command = read_block_category_header(&mut reader).unwrap();
        let distance = read_block_category_header(&mut reader).unwrap();

        assert_eq!(literal.types(), 1);
        assert_eq!(command.types(), 1);
        assert_eq!(distance.types(), 1);
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

    #[test]
    fn distance_symbol_zero_does_not_update_ring() {
        let mut output = b"0123456789abcdef".to_vec();
        let mut distances = DistanceRing::new();

        copy_from_distance(
            &mut output,
            CopyRequest {
                meta_block_start: 0,
                needed: 20,
                window_size: 1 << 16,
                output_base: 0,
                distance: 4,
                len: 4,
                push_distance: false,
            },
            &mut distances,
        )
        .unwrap();

        let mut reader = BitReader::new(&[]);
        assert_eq!(read_distance(&mut reader, 1, 0, 0, &distances).unwrap(), 11);
    }

    #[test]
    fn command_distance_context_comes_from_command_prefix() {
        assert_eq!(command_code_parts(0).unwrap().3, 0);
        assert_eq!(command_code_parts(2).unwrap().3, 2);
        assert_eq!(command_code_parts(3).unwrap().3, 3);
        assert_eq!(command_code_parts(64).unwrap().3, 3);
        assert_eq!(command_code_parts(256 + 2).unwrap().3, 2);
    }

    #[test]
    fn literal_context_uses_zero_second_previous_byte_until_two_bytes_exist() {
        assert_eq!(literal_context_for_mode(b"\r", 3).unwrap(), 8);
        assert_eq!(literal_context_for_mode(b"\r\n", 3).unwrap(), 9);
    }
}

#[cfg(kani)]
mod verification {
    use super::*;

    #[kani::proof]
    fn default_short_distances_are_non_zero() {
        let symbol = kani::any::<u8>();
        kani::assume(symbol < 16);

        let distance = DistanceRing::new().resolve(usize::from(symbol)).unwrap();

        assert!(distance > 0);
    }

    #[kani::proof]
    fn backward_copy_bound_check_caps_end_at_needed() {
        let produced = kani::any::<u8>();
        let distance = kani::any::<u8>();
        let len = kani::any::<u8>();
        kani::assume((1..=8).contains(&produced));
        kani::assume((1..=produced).contains(&distance));
        kani::assume(len <= 8);

        let produced = usize::from(produced);
        let request = CopyRequest {
            meta_block_start: 0,
            needed: produced + usize::from(len),
            window_size: 16,
            output_base: 0,
            distance: usize::from(distance),
            len: usize::from(len),
            push_distance: true,
        };

        let end = checked_backward_copy_end(produced, request).unwrap();

        assert_eq!(end, request.needed);
        assert!(end <= request.needed);
    }
}
