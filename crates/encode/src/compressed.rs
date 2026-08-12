use alloc::{vec, vec::Vec};

use burli_core::{
    BurliError, CompressError, Options,
    bits::BitWriter,
    format::{MAX_WINDOW_BITS, MIN_BLOCK_BITS, MIN_WINDOW_BITS},
};

const MAX_LITERAL_ONLY_QUALITY: u8 = 5;
const MAX_META_BLOCK_SIZE: usize = 1 << 24;
const MIN_MATCH_BYTES: usize = 4;
const LITERAL_ALPHABET_SIZE: usize = 256;
const COMMAND_ALPHABET_SIZE: usize = 704;
const CODE_LENGTH_ALPHABET_SIZE: usize = 18;
const MAX_CODE_BITS: u8 = 15;
const CODE_LENGTH_ORDER: [u8; 18] = [1, 2, 3, 4, 0, 5, 17, 6, 16, 7, 8, 9, 10, 11, 12, 13, 14, 15];
const MAX_SIMPLE_PREFIX_SYMBOLS: usize = 4;
const INITIAL_LAST_DISTANCE: usize = 4;

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

    write_compressed_meta_blocks(&mut writer, input, options)?;
    write_last_empty_meta_block(&mut writer)?;

    let compressed = writer.into_bytes();
    let stored_options = options.clone().quality(0)?;
    let stored = crate::stored::compress_stored_with_options(input, &stored_options)?;
    if stored.len() < compressed.len() {
        Ok(stored)
    } else {
        Ok(compressed)
    }
}

#[cfg(feature = "std")]
pub(crate) fn write_stream_header(
    writer: &mut BitWriter,
    options: &Options,
) -> Result<(), CompressError> {
    if options.quality_value() > MAX_LITERAL_ONLY_QUALITY {
        return Err(BurliError::Unsupported(
            "only q0..q5 Brotli encoding is implemented yet",
        ));
    }
    write_window_bits(writer, options.window_bits_value())
}

#[cfg(feature = "std")]
pub(crate) fn write_stream_chunk(
    writer: &mut BitWriter,
    input: &[u8],
    options: &Options,
) -> Result<(), CompressError> {
    if input.is_empty() {
        return Ok(());
    }
    if options.quality_value() > MAX_LITERAL_ONLY_QUALITY {
        return Err(BurliError::Unsupported(
            "only q0..q5 Brotli encoding is implemented yet",
        ));
    }
    let max_backward_distance = (1_usize << options.window_bits_value()) - 16;
    write_compressed_chunk(
        writer,
        input,
        options.quality_value(),
        max_backward_distance.min(input.len()),
    )
}

fn max_literal_only_size(input_len: usize) -> usize {
    input_len
        .saturating_add(input_len / 1024)
        .saturating_add(256)
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

fn write_compressed_meta_blocks(
    writer: &mut BitWriter,
    input: &[u8],
    options: &Options,
) -> Result<(), CompressError> {
    let block_size = match options.block_bits_value() {
        Some(bits) => 1_usize << bits,
        None if options.quality_value() == 0 => MAX_META_BLOCK_SIZE,
        None => 1_usize << MIN_BLOCK_BITS,
    }
    .min(MAX_META_BLOCK_SIZE);
    let max_backward_distance = (1_usize << options.window_bits_value()) - 16;

    for chunk in input.chunks(block_size) {
        write_compressed_chunk(
            writer,
            chunk,
            options.quality_value(),
            max_backward_distance.min(chunk.len()),
        )?;
    }

    Ok(())
}

fn write_compressed_chunk(
    writer: &mut BitWriter,
    input: &[u8],
    quality: u8,
    max_backward_distance: usize,
) -> Result<(), CompressError> {
    if input.len() < min_match_len(quality) {
        return write_compressed_literal_meta_block(writer, input);
    }

    let tokens = collect_tokens(input, quality, max_backward_distance);
    if tokens.iter().all(|token| token.copy_len == 0) {
        return write_compressed_literal_meta_block(writer, input);
    }
    write_token_batches(writer, input, &tokens)
}

#[derive(Clone, Copy, Debug)]
struct Token {
    insert_start: usize,
    insert_len: usize,
    copy_len: usize,
    distance: usize,
    use_last_distance: bool,
}

impl Token {
    const fn is_copy(self) -> bool {
        self.copy_len != 0
    }

    const fn block_len(self) -> usize {
        self.insert_len + self.copy_len
    }
}

fn collect_tokens(input: &[u8], quality: u8, max_backward_distance: usize) -> Vec<Token> {
    let table_size = hash_table_size(quality);
    let mut table = vec![usize::MAX; table_size];
    let table_mask = table_size - 1;
    let min_match = min_match_len(quality);
    let mut tokens = Vec::new();
    let mut pos = 0;
    let mut insert_start = 0;

    while pos + MIN_MATCH_BYTES <= input.len() {
        let key = hash_key(input, pos, quality) & table_mask;
        let previous = table[key];
        table[key] = pos;

        if previous != usize::MAX
            && pos - previous <= max_backward_distance
            && pos + min_match <= input.len()
            && input[previous..previous + MIN_MATCH_BYTES] == input[pos..pos + MIN_MATCH_BYTES]
        {
            let max_copy_len = (MAX_META_BLOCK_SIZE - (pos - insert_start)).min(input.len() - pos);
            let copy_len = match_len(input, previous, pos, max_copy_len);
            if copy_len >= min_match {
                tokens.push(Token {
                    insert_start,
                    insert_len: pos - insert_start,
                    copy_len,
                    distance: pos - previous,
                    use_last_distance: false,
                });
                store_match_range(
                    input,
                    quality,
                    &mut table,
                    table_mask,
                    pos + 1,
                    copy_len.saturating_sub(1),
                );
                pos += copy_len;
                insert_start = pos;
                continue;
            }
        }

        pos += 1;
    }

    if insert_start < input.len() {
        tokens.push(Token {
            insert_start,
            insert_len: input.len() - insert_start,
            copy_len: 0,
            distance: 0,
            use_last_distance: false,
        });
    }

    mark_last_distance_tokens(&mut tokens);
    tokens
}

fn store_match_range(
    input: &[u8],
    quality: u8,
    table: &mut [usize],
    table_mask: usize,
    start: usize,
    copy_len: usize,
) {
    let end = start
        .saturating_add(copy_len)
        .min(input.len().saturating_sub(MIN_MATCH_BYTES - 1));
    for pos in start..end {
        let key = hash_key(input, pos, quality) & table_mask;
        table[key] = pos;
    }
}

fn mark_last_distance_tokens(tokens: &mut [Token]) {
    let mut last_distance = INITIAL_LAST_DISTANCE;
    for token in tokens {
        if !token.is_copy() {
            continue;
        }
        token.use_last_distance =
            token.distance == last_distance && token_supports_last_distance(*token);
        last_distance = token.distance;
    }
}

fn token_supports_last_distance(token: Token) -> bool {
    let Ok(insert) = insert_length_code(token.insert_len) else {
        return false;
    };
    let Ok(copy) = copy_length_code(token.copy_len) else {
        return false;
    };
    insert.code < 8 && copy.code < 16
}

fn write_token_batches(
    writer: &mut BitWriter,
    input: &[u8],
    tokens: &[Token],
) -> Result<(), CompressError> {
    let mut start = 0;
    while start < tokens.len() {
        let end = token_batch_end(&tokens[start..]) + start;
        write_token_batch(writer, input, &tokens[start..end])?;
        start = end;
    }
    Ok(())
}

fn token_batch_end(tokens: &[Token]) -> usize {
    let mut block_len = 0_usize;

    for (index, &token) in tokens.iter().enumerate() {
        if token_command_symbol(token).is_err() {
            return index.max(1);
        }
        if token.is_copy() && distance_code(token.distance).is_err() {
            return index.max(1);
        }
        let next_len = block_len.saturating_add(token.block_len());
        if next_len > MAX_META_BLOCK_SIZE {
            return index.max(1);
        }

        block_len = next_len;
    }

    tokens.len()
}

fn push_unique(symbols: &mut Vec<u16>, symbol: u16) {
    if !symbols.contains(&symbol) {
        symbols.push(symbol);
    }
}

fn min_match_len(quality: u8) -> usize {
    match quality {
        0 => 5,
        1 => 64,
        2 => 48,
        3 => 32,
        4 => 24,
        _ => 16,
    }
}

fn hash_table_size(quality: u8) -> usize {
    1_usize
        << match quality {
            0 | 4 => 15,
            1 => 12,
            2 => 13,
            3 => 14,
            _ => 16,
        }
}

fn hash4(input: &[u8], pos: usize) -> usize {
    let word = u32::from_le_bytes([input[pos], input[pos + 1], input[pos + 2], input[pos + 3]]);
    ((word.wrapping_mul(0x1e35_a7bd)) >> 16) as usize
}

fn hash_key(input: &[u8], pos: usize, quality: u8) -> usize {
    if quality == 0 && pos + 8 <= input.len() {
        let word = u64::from_le_bytes([
            input[pos],
            input[pos + 1],
            input[pos + 2],
            input[pos + 3],
            input[pos + 4],
            input[pos + 5],
            input[pos + 6],
            input[pos + 7],
        ]);
        return (((word << 24).wrapping_mul(0x1e35_a7bd)) >> 49) as usize;
    }
    hash4(input, pos)
}

fn match_len(input: &[u8], previous: usize, pos: usize, max_len: usize) -> usize {
    let mut len = 0;
    while len < max_len && input[previous + len] == input[pos + len] {
        len += 1;
    }
    len
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
    write_block_and_context_header(writer)?;
    let literal_symbols = input
        .iter()
        .map(|&literal| u16::from(literal))
        .collect::<Vec<_>>();
    let literal_codes =
        write_prefix_code_from_symbols(writer, LITERAL_ALPHABET_SIZE, &literal_symbols)?;
    write_simple_prefix_code_single(writer, COMMAND_ALPHABET_SIZE, command_symbol)?;
    write_simple_prefix_code_single(writer, 64, 0)?;
    writer.write_bits(insert.extra_bits, insert.extra)?;
    for &literal in input {
        write_literal(writer, &literal_codes, literal)?;
    }
    Ok(())
}

fn write_token_batch(
    writer: &mut BitWriter,
    input: &[u8],
    tokens: &[Token],
) -> Result<(), CompressError> {
    let block_len = tokens.iter().map(|token| token.block_len()).sum::<usize>();
    if block_len == 0 || block_len > MAX_META_BLOCK_SIZE {
        return Err(BurliError::Format("invalid compressed Brotli block size"));
    }

    let mut literal_symbols = Vec::new();
    let mut command_symbols = Vec::new();
    let mut distance_symbols = Vec::new();
    for &token in tokens {
        literal_symbols.extend(
            input[token.insert_start..token.insert_start + token.insert_len]
                .iter()
                .map(|&literal| u16::from(literal)),
        );
        command_symbols.push(token_command_symbol(token)?);
        if token.is_copy() && !token.use_last_distance {
            distance_symbols.push(distance_code(token.distance)?.symbol);
        }
    }
    if distance_symbols.is_empty() {
        distance_symbols.push(0);
    }

    write_meta_block_len(writer, block_len)?;
    write_block_and_context_header(writer)?;
    let literal_codes =
        write_prefix_code_from_symbols(writer, LITERAL_ALPHABET_SIZE, &literal_symbols)?;
    let command_codes =
        write_prefix_code_from_symbols(writer, COMMAND_ALPHABET_SIZE, &command_symbols)?;
    let distance_codes = write_prefix_code_from_symbols(writer, 64, &distance_symbols)?;

    for &token in tokens {
        let insert = insert_length_code(token.insert_len)?;
        let copy = if token.is_copy() {
            Some(copy_length_code(token.copy_len)?)
        } else {
            None
        };
        let command_symbol = token_command_symbol(token)?;
        let command_code = symbol_code(&command_codes, command_symbol)?;
        writer.write_bits(command_code.len, u64::from(command_code.bits))?;
        writer.write_bits(insert.extra_bits, insert.extra)?;
        if let Some(copy) = copy {
            writer.write_bits(copy.extra_bits, copy.extra)?;
        }

        for &literal in &input[token.insert_start..token.insert_start + token.insert_len] {
            write_literal(writer, &literal_codes, literal)?;
        }

        if token.is_copy() && !token.use_last_distance {
            let distance = distance_code(token.distance)?;
            let distance_code = symbol_code(&distance_codes, distance.symbol)?;
            writer.write_bits(distance_code.len, u64::from(distance_code.bits))?;
            writer.write_bits(distance.extra_bits, distance.extra)?;
        }
    }

    Ok(())
}

fn write_block_and_context_header(writer: &mut BitWriter) -> Result<(), CompressError> {
    write_var_len_u8(writer, 0)?;
    write_var_len_u8(writer, 0)?;
    write_var_len_u8(writer, 0)?;
    writer.write_bits(2, 0)?;
    writer.write_bits(4, 0)?;
    writer.write_bits(2, 0)?;
    write_var_len_u8(writer, 0)?;
    write_var_len_u8(writer, 0)
}

fn write_meta_block_len(writer: &mut BitWriter, len: usize) -> Result<(), CompressError> {
    if len == 0 || len > MAX_META_BLOCK_SIZE {
        return Err(BurliError::Format("invalid compressed Brotli block size"));
    }

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

fn write_code_length_code_len(writer: &mut BitWriter, len: u8) -> Result<(), CompressError> {
    match len {
        0 => writer.write_bits(2, 0),
        1 => writer.write_bits(4, 7),
        2 => writer.write_bits(3, 3),
        3 => writer.write_bits(2, 2),
        4 => writer.write_bits(2, 1),
        5 => writer.write_bits(4, 15),
        _ => Err(BurliError::Format("unsupported Brotli code length code")),
    }
}

fn write_prefix_code_from_symbols(
    writer: &mut BitWriter,
    alphabet_size: usize,
    symbols: &[u16],
) -> Result<Vec<SymbolCode>, CompressError> {
    if symbols.is_empty() {
        return write_simple_prefix_code_symbols(writer, alphabet_size, &[0]);
    }

    let mut frequencies = vec![0_usize; alphabet_size];
    for &symbol in symbols {
        let index = usize::from(symbol);
        if index >= alphabet_size {
            return Err(BurliError::Format("Brotli prefix symbol exceeds alphabet"));
        }
        frequencies[index] += 1;
    }

    let mut used = frequencies
        .iter()
        .enumerate()
        .filter_map(|(symbol, &frequency)| (frequency != 0).then_some((symbol as u16, frequency)))
        .collect::<Vec<_>>();
    if used.len() <= MAX_SIMPLE_PREFIX_SYMBOLS {
        let symbols = used.iter().map(|&(symbol, _)| symbol).collect::<Vec<_>>();
        return write_simple_prefix_code_symbols(writer, alphabet_size, &symbols);
    }

    let lengths = huffman_code_lengths(&frequencies, MAX_CODE_BITS)
        .unwrap_or_else(|| balanced_code_lengths(alphabet_size, &mut used, MAX_CODE_BITS));

    write_complex_prefix_code_lengths(writer, &lengths)?;
    Ok(symbol_codes_from_lengths(&lengths))
}

fn balanced_code_lengths(alphabet_size: usize, used: &mut [(u16, usize)], max_bits: u8) -> Vec<u8> {
    used.sort_by(
        |&(left_symbol, left_frequency), &(right_symbol, right_frequency)| {
            right_frequency
                .cmp(&left_frequency)
                .then_with(|| left_symbol.cmp(&right_symbol))
        },
    );

    let mut lengths = vec![0_u8; alphabet_size];
    let base_bits = ceil_log2(used.len()).unwrap().min(max_bits);
    let short_count = (1_usize << base_bits) - used.len();
    for (rank, &(symbol, _)) in used.iter().enumerate() {
        lengths[usize::from(symbol)] = if rank < short_count {
            base_bits - 1
        } else {
            base_bits
        };
    }
    lengths
}

#[derive(Clone, Debug)]
struct HuffmanNode {
    frequency: u64,
    min_symbol: u16,
    parent: Option<usize>,
}

fn huffman_code_lengths(frequencies: &[usize], max_bits: u8) -> Option<Vec<u8>> {
    let mut nodes = Vec::new();
    let mut active = Vec::new();
    let mut leaves = Vec::new();

    for (symbol, &frequency) in frequencies.iter().enumerate() {
        if frequency == 0 {
            continue;
        }
        let index = nodes.len();
        nodes.push(HuffmanNode {
            frequency: frequency as u64,
            min_symbol: symbol as u16,
            parent: None,
        });
        active.push(index);
        leaves.push((symbol, index));
    }

    if leaves.len() <= 1 {
        return None;
    }

    while active.len() > 1 {
        active.sort_by(|&left, &right| {
            nodes[left]
                .frequency
                .cmp(&nodes[right].frequency)
                .then_with(|| nodes[left].min_symbol.cmp(&nodes[right].min_symbol))
        });
        let first = active.remove(0);
        let second = active.remove(0);
        let parent = nodes.len();
        nodes.push(HuffmanNode {
            frequency: nodes[first].frequency + nodes[second].frequency,
            min_symbol: nodes[first].min_symbol.min(nodes[second].min_symbol),
            parent: None,
        });
        nodes[first].parent = Some(parent);
        nodes[second].parent = Some(parent);
        active.push(parent);
    }

    let mut lengths = vec![0_u8; frequencies.len()];
    for (symbol, node_index) in leaves {
        let mut depth = 0_u8;
        let mut cursor = node_index;
        while let Some(parent) = nodes[cursor].parent {
            depth = depth.checked_add(1)?;
            cursor = parent;
        }
        if depth == 0 || depth > max_bits {
            return None;
        }
        lengths[symbol] = depth;
    }

    Some(lengths)
}

fn write_complex_prefix_code_lengths(
    writer: &mut BitWriter,
    lengths: &[u8],
) -> Result<(), CompressError> {
    writer.write_bits(2, 0)?;

    let mut length_frequencies = [0_usize; CODE_LENGTH_ALPHABET_SIZE];
    for &len in lengths {
        if len > MAX_CODE_BITS {
            return Err(BurliError::Format("Brotli Huffman code length exceeds 15"));
        }
        length_frequencies[usize::from(len)] += 1;
    }

    let mut used_lengths = length_frequencies
        .iter()
        .enumerate()
        .filter_map(|(symbol, &frequency)| (frequency != 0).then_some((symbol as u16, frequency)))
        .collect::<Vec<_>>();
    used_lengths.sort_by(
        |&(left_symbol, left_frequency), &(right_symbol, right_frequency)| {
            right_frequency
                .cmp(&left_frequency)
                .then_with(|| left_symbol.cmp(&right_symbol))
        },
    );

    let mut code_length_lengths = [0_u8; CODE_LENGTH_ALPHABET_SIZE];
    if used_lengths.len() == 1 {
        code_length_lengths[usize::from(used_lengths[0].0)] = 1;
    } else {
        let base_bits = ceil_log2(used_lengths.len())?;
        let short_count = (1_usize << base_bits) - used_lengths.len();
        for (rank, &(symbol, _)) in used_lengths.iter().enumerate() {
            code_length_lengths[usize::from(symbol)] = if rank < short_count {
                base_bits - 1
            } else {
                base_bits
            };
        }
    }

    let entries_to_write = code_length_entries_to_write(&code_length_lengths);
    for &symbol in CODE_LENGTH_ORDER.iter().take(entries_to_write) {
        write_code_length_code_len(writer, code_length_lengths[usize::from(symbol)])?;
    }

    if used_lengths.len() != 1 {
        let code_length_codes = symbol_codes_from_lengths(&code_length_lengths);
        let entries_to_write = lengths
            .iter()
            .rposition(|&len| len != 0)
            .map_or(lengths.len(), |index| index + 1);
        for &len in lengths.iter().take(entries_to_write) {
            let code = symbol_code(&code_length_codes, u16::from(len))?;
            writer.write_bits(code.len, u64::from(code.bits))?;
        }
    }

    Ok(())
}

fn code_length_entries_to_write(code_length_lengths: &[u8; CODE_LENGTH_ALPHABET_SIZE]) -> usize {
    let non_zero = code_length_lengths.iter().filter(|&&len| len != 0).count();
    if non_zero <= 1 {
        return CODE_LENGTH_ORDER.len();
    }

    CODE_LENGTH_ORDER
        .iter()
        .rposition(|&symbol| code_length_lengths[usize::from(symbol)] != 0)
        .map_or(CODE_LENGTH_ORDER.len(), |index| index + 1)
}

fn ceil_log2(value: usize) -> Result<u8, CompressError> {
    if value == 0 {
        return Err(BurliError::Format("invalid Brotli prefix symbol count"));
    }
    if value == 1 {
        return Ok(0);
    }
    Ok((usize::BITS - (value - 1).leading_zeros()) as u8)
}

fn write_literal(
    writer: &mut BitWriter,
    codes: &[SymbolCode],
    literal: u8,
) -> Result<(), CompressError> {
    let code = symbol_code(codes, u16::from(literal))?;
    writer.write_bits(code.len, u64::from(code.bits))
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

#[derive(Clone, Copy, Debug)]
struct CopyLengthCode {
    code: usize,
    extra_bits: u8,
    extra: u64,
}

fn copy_length_code(len: usize) -> Result<CopyLengthCode, CompressError> {
    for code in 0..=23 {
        let (base, extra_bits) = copy_length_prefix(code)?;
        let span = 1_usize << extra_bits;
        if (base..base + span).contains(&len) {
            return Ok(CopyLengthCode {
                code,
                extra_bits,
                extra: (len - base) as u64,
            });
        }
    }
    Err(BurliError::Format("Brotli copy length exceeds range"))
}

fn copy_length_prefix(code: usize) -> Result<(usize, u8), CompressError> {
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

fn command_symbol_for_insert(insert_code: usize) -> Result<u16, CompressError> {
    let symbol = match insert_code {
        0..=7 => insert_code * 8,
        8..=15 => 256 + (insert_code - 8) * 8,
        16..=23 => 448 + (insert_code - 16) * 8,
        _ => return Err(BurliError::Format("invalid Brotli insert length code")),
    };
    Ok(symbol as u16)
}

fn command_symbol_for_insert_copy(
    insert_code: usize,
    copy_code: usize,
    use_last_distance: bool,
) -> Result<u16, CompressError> {
    let insert_group = insert_code / 8;
    let copy_group = copy_code / 8;
    let insert_low = insert_code % 8;
    let copy_low = copy_code % 8;
    let cell = match (insert_group, copy_group) {
        (0, 0) => 2,
        (0, 1) => 3,
        (1, 0) => 4,
        (1, 1) => 5,
        (0, 2) => 6,
        (2, 0) => 7,
        (1, 2) => 8,
        (2, 1) => 9,
        (2, 2) => 10,
        _ => return Err(BurliError::Format("invalid Brotli command length code")),
    };
    let cell = if use_last_distance {
        match cell {
            2 => 0,
            3 => 1,
            _ => return Err(BurliError::Format("invalid Brotli last-distance command")),
        }
    } else {
        cell
    };
    Ok((cell * 64 + insert_low * 8 + copy_low) as u16)
}

fn token_command_symbol(token: Token) -> Result<u16, CompressError> {
    let insert = insert_length_code(token.insert_len)?;
    if !token.is_copy() {
        return command_symbol_for_insert(insert.code);
    }
    let copy = copy_length_code(token.copy_len)?;
    command_symbol_for_insert_copy(insert.code, copy.code, token.use_last_distance)
}

#[derive(Clone, Copy, Debug)]
struct DistanceCode {
    symbol: u16,
    extra_bits: u8,
    extra: u64,
}

fn distance_code(distance: usize) -> Result<DistanceCode, CompressError> {
    if distance == 0 {
        return Err(BurliError::Format("invalid Brotli zero distance"));
    }

    for bits in 1..=24 {
        for parity in 0..=1 {
            let base = ((2 + parity) << bits) - 3;
            let span = 1_usize << bits;
            if (base..base + span).contains(&distance) {
                return Ok(DistanceCode {
                    symbol: (16 + 2 * (bits - 1) + parity) as u16,
                    extra_bits: bits as u8,
                    extra: (distance - base) as u64,
                });
            }
        }
    }

    Err(BurliError::Format("Brotli distance exceeds range"))
}

#[cfg(kani)]
fn reverse_bits(value: u8, width: u8) -> u8 {
    let mut reversed = 0;
    for bit in 0..width {
        reversed <<= 1;
        reversed |= (value >> bit) & 1;
    }
    reversed
}

fn reverse_bits_u16(value: u16, width: u8) -> u16 {
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

#[derive(Clone, Copy, Debug)]
struct SymbolCode {
    symbol: u16,
    len: u8,
    bits: u16,
}

fn write_simple_prefix_code_symbols(
    writer: &mut BitWriter,
    alphabet_size: usize,
    symbols: &[u16],
) -> Result<Vec<SymbolCode>, CompressError> {
    let symbols = sorted_unique_symbols(symbols, alphabet_size)?;
    let alphabet_bits = alphabet_bits(alphabet_size);

    writer.write_bits(2, 1)?;
    writer.write_bits(2, (symbols.len() - 1) as u64)?;
    for &symbol in &symbols {
        writer.write_bits(alphabet_bits, u64::from(symbol))?;
    }
    if symbols.len() == 4 {
        writer.write_bits(1, 0)?;
    }

    Ok(simple_symbol_codes(&symbols))
}

fn sorted_unique_symbols(symbols: &[u16], alphabet_size: usize) -> Result<Vec<u16>, CompressError> {
    if symbols.is_empty() || symbols.len() > MAX_SIMPLE_PREFIX_SYMBOLS {
        return Err(BurliError::Format(
            "invalid Brotli simple prefix symbol count",
        ));
    }

    let mut unique = Vec::new();
    for &symbol in symbols {
        if usize::from(symbol) >= alphabet_size {
            return Err(BurliError::Format("Brotli prefix symbol exceeds alphabet"));
        }
        push_unique(&mut unique, symbol);
    }
    unique.sort_unstable();
    Ok(unique)
}

fn simple_symbol_codes(symbols: &[u16]) -> Vec<SymbolCode> {
    let lengths = match symbols.len() {
        1 => vec![0],
        2 => vec![1, 1],
        3 => vec![1, 2, 2],
        4 => vec![2, 2, 2, 2],
        _ => unreachable!(),
    };
    symbol_codes_from_lengths_and_symbols(&lengths, symbols)
}

fn symbol_codes_from_lengths(lengths: &[u8]) -> Vec<SymbolCode> {
    let symbols = lengths
        .iter()
        .enumerate()
        .filter_map(|(symbol, &len)| (len != 0).then_some(symbol as u16))
        .collect::<Vec<_>>();
    let lengths = symbols
        .iter()
        .map(|&symbol| lengths[usize::from(symbol)])
        .collect::<Vec<_>>();
    symbol_codes_from_lengths_and_symbols(&lengths, &symbols)
}

fn symbol_codes_from_lengths_and_symbols(lengths: &[u8], symbols: &[u16]) -> Vec<SymbolCode> {
    let mut counts = [0_u16; 16];
    for &len in lengths {
        if len != 0 {
            counts[usize::from(len)] += 1;
        }
    }

    let mut next_code = [0_u16; 16];
    let mut code = 0_u16;
    for bits in 1..=15 {
        code = (code + counts[bits - 1]) << 1;
        next_code[bits] = code;
    }

    let mut codes = Vec::with_capacity(symbols.len());
    for (&symbol, &len) in symbols.iter().zip(lengths) {
        let code = if len == 0 {
            0
        } else {
            let code = next_code[usize::from(len)];
            next_code[usize::from(len)] += 1;
            code
        };
        codes.push(SymbolCode {
            symbol,
            len,
            bits: reverse_bits_u16(code, len),
        });
    }
    codes
}

fn symbol_code(codes: &[SymbolCode], symbol: u16) -> Result<SymbolCode, CompressError> {
    codes
        .iter()
        .find(|code| code.symbol == symbol)
        .copied()
        .ok_or(BurliError::Format("missing Brotli prefix symbol"))
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
        let input =
            b"abcdefghijklmnopqrstuvwxyz0123456789abcdefghijklmnopqrstuvwxyz0123456789".repeat(64);
        let encoded =
            compress_with_options(&input, &Options::default().quality(1).unwrap()).unwrap();

        assert_ne!(
            encoded,
            crate::stored::compress_stored_with_options(
                &input,
                &Options::default().quality(0).unwrap()
            )
            .unwrap()
        );
        assert_eq!(burli_decode::decompress(&encoded).unwrap(), input);
    }

    #[test]
    fn q0_emits_compressed_stream_for_repeated_payload() {
        let input = b"function demo(){return demo_value;} ".repeat(256);
        let encoded =
            compress_with_options(&input, &Options::default().quality(0).unwrap()).unwrap();
        let stored = crate::stored::compress_stored_with_options(
            &input,
            &Options::default().quality(0).unwrap(),
        )
        .unwrap();

        assert!(encoded.len() < stored.len());
        assert_eq!(burli_decode::decompress(&encoded).unwrap(), input);
    }

    #[test]
    fn q0_compresses_long_repetitive_payload() {
        let input = br#"{"name":"burli","kind":"brotli","safe":true}"#.repeat(4096);
        let encoded =
            compress_with_options(&input, &Options::default().quality(0).unwrap()).unwrap();

        assert!(encoded.len() * 20 < input.len());
        assert_eq!(burli_decode::decompress(&encoded).unwrap(), input);
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

    #[test]
    fn q5_round_trips_literal_run_above_64k() {
        let input = vec![b'x'; 70_000];
        let encoded =
            compress_with_options(&input, &Options::default().quality(5).unwrap()).unwrap();

        assert_eq!(burli_decode::decompress(&encoded).unwrap(), input);
    }

    #[test]
    fn q5_compresses_repeated_payload() {
        let input = b"0123456789abcdef".repeat(128);
        let encoded =
            compress_with_options(&input, &Options::default().quality(5).unwrap()).unwrap();

        assert!(encoded.len() < input.len());
        assert_eq!(burli_decode::decompress(&encoded).unwrap(), input);
    }

    #[test]
    fn q1_compresses_long_repeated_payload() {
        let input =
            b"abcdefghijklmnopqrstuvwxyz0123456789abcdefghijklmnopqrstuvwxyz0123456789".repeat(64);
        let encoded =
            compress_with_options(&input, &Options::default().quality(1).unwrap()).unwrap();

        assert!(encoded.len() < input.len());
        assert_eq!(burli_decode::decompress(&encoded).unwrap(), input);
    }
}

#[cfg(kani)]
mod verification {
    use super::*;

    #[kani::proof]
    #[kani::unwind(9)]
    fn reverse_bits_width_8_is_involution() {
        let value = kani::any::<u8>();

        assert_eq!(reverse_bits(reverse_bits(value, 8), 8), value);
    }

    #[kani::proof]
    #[kani::unwind(25)]
    fn insert_length_code_covers_meta_block_range() {
        let raw_len = kani::any::<u32>();
        kani::assume(raw_len > 0);
        kani::assume(raw_len <= MAX_META_BLOCK_SIZE as u32);

        let len = raw_len as usize;
        let insert = insert_length_code(len).unwrap();
        let (base, extra_bits) = insert_length_prefix(insert.code).unwrap();
        let command_symbol = command_symbol_for_insert(insert.code).unwrap();

        assert_eq!(base + insert.extra as usize, len);
        assert!(insert.extra < (1_u64 << extra_bits));
        assert!(usize::from(command_symbol) < 704);
        assert_eq!(decode_insert_code(command_symbol), insert.code);
    }

    fn decode_insert_code(symbol: u16) -> usize {
        let code = usize::from(symbol);
        let high = (code >> 3) & 0b111;
        match code >> 6 {
            0 => high,
            4 => 8 + high,
            7 => 16 + high,
            _ => usize::MAX,
        }
    }
}
