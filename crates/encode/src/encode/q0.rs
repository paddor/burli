use alloc::{boxed::Box, vec, vec::Vec};

use burli_core::{BurliError, CompressError, bits::BitWriter};

use super::{
    COMMAND_ALPHABET_SIZE, INITIAL_LAST_DISTANCE, LITERAL_ALPHABET_SIZE, MAX_META_BLOCK_SIZE,
    Token, command_symbol_for_insert, command_symbol_for_insert_copy, copy_length_code,
    distance_code, hash_word_q0, insert_length_code, is_match5, match_len, next_hash_word,
    read_u64_le, symbol_code, symbol_code_map, token_supports_last_distance,
    write_block_and_context_header, write_literal, write_meta_block_len,
    write_prefix_code_from_frequencies,
};

const TABLE_SIZE: usize = 1 << 15;
const TABLE_MASK: usize = TABLE_SIZE - 1;
const HASH_SHIFT: usize = 49;
const EMPTY: u32 = u32::MAX;
const NO_DISTANCE: u16 = u16::MAX;

#[derive(Clone, Debug)]
pub(super) struct Batch {
    records: Vec<Record>,
    literal_frequencies: [usize; LITERAL_ALPHABET_SIZE],
    command_frequencies: [usize; COMMAND_ALPHABET_SIZE],
    distance_frequencies: [usize; 64],
    has_distance: bool,
    has_copy: bool,
}

impl Batch {
    fn with_capacity(capacity: usize) -> Self {
        Self {
            records: Vec::with_capacity(capacity),
            literal_frequencies: [0; LITERAL_ALPHABET_SIZE],
            command_frequencies: [0; COMMAND_ALPHABET_SIZE],
            distance_frequencies: [0; 64],
            has_distance: false,
            has_copy: false,
        }
    }

    #[inline]
    pub(super) const fn has_copy(&self) -> bool {
        self.has_copy
    }

    fn push(&mut self, input: &[u8], token: Token) -> Result<(), CompressError> {
        let insert = insert_length_code(token.insert_len)?;
        let copy = if token.is_copy() {
            Some(copy_length_code(token.copy_len)?)
        } else {
            None
        };
        let command_symbol = if let Some(copy) = copy {
            command_symbol_for_insert_copy(insert.code, copy.code, token.use_last_distance)?
        } else {
            command_symbol_for_insert(insert.code)?
        };
        let distance = if token.is_copy() && !token.use_last_distance {
            Some(distance_code(token.distance)?)
        } else {
            None
        };

        for &literal in &input[token.insert_start..token.insert_start + token.insert_len] {
            self.literal_frequencies[usize::from(literal)] += 1;
        }
        self.command_frequencies[usize::from(command_symbol)] += 1;
        if let Some(distance) = distance {
            self.distance_frequencies[usize::from(distance.symbol)] += 1;
            self.has_distance = true;
        }
        self.has_copy |= token.is_copy();
        self.records.push(Record {
            insert_start: token.insert_start as u32,
            insert_len: token.insert_len as u32,
            insert_extra: insert.extra as u32,
            copy_extra: copy.map_or(0, |copy| copy.extra as u32),
            distance_extra: distance.map_or(0, |distance| distance.extra as u32),
            command_symbol,
            distance_symbol: distance.map_or(NO_DISTANCE, |distance| distance.symbol),
            insert_extra_bits: insert.extra_bits,
            copy_extra_bits: copy.map_or(0, |copy| copy.extra_bits),
            distance_extra_bits: distance.map_or(0, |distance| distance.extra_bits),
        });
        Ok(())
    }

    fn ensure_distance_frequencies(&mut self) {
        if !self.has_distance {
            self.distance_frequencies[0] = 1;
        }
    }

    pub(super) fn write(
        &self,
        writer: &mut BitWriter,
        input: &[u8],
        block_len: usize,
    ) -> Result<(), CompressError> {
        if block_len == 0 || block_len > MAX_META_BLOCK_SIZE {
            return Err(BurliError::Format("invalid compressed Brotli block size"));
        }

        write_meta_block_len(writer, block_len)?;
        write_block_and_context_header(writer)?;
        let literal_codes = write_prefix_code_from_frequencies(
            writer,
            LITERAL_ALPHABET_SIZE,
            &self.literal_frequencies,
        )?;
        let command_codes = write_prefix_code_from_frequencies(
            writer,
            COMMAND_ALPHABET_SIZE,
            &self.command_frequencies,
        )?;
        let distance_codes =
            write_prefix_code_from_frequencies(writer, 64, &self.distance_frequencies)?;
        let literal_code_map = symbol_code_map(&literal_codes, LITERAL_ALPHABET_SIZE);
        let command_code_map = symbol_code_map(&command_codes, COMMAND_ALPHABET_SIZE);
        let distance_code_map = symbol_code_map(&distance_codes, 64);

        for record in &self.records {
            let command_code = symbol_code(&command_code_map, record.command_symbol)?;
            writer.write_bits_trusted(command_code.len, u64::from(command_code.bits));
            writer.write_bits_trusted(record.insert_extra_bits, u64::from(record.insert_extra));
            writer.write_bits_trusted(record.copy_extra_bits, u64::from(record.copy_extra));

            let insert_start = record.insert_start as usize;
            let insert_end = insert_start + record.insert_len as usize;
            for &literal in &input[insert_start..insert_end] {
                write_literal(writer, &literal_code_map, literal)?;
            }

            if record.distance_symbol != NO_DISTANCE {
                let distance_code = symbol_code(&distance_code_map, record.distance_symbol)?;
                writer.write_bits_trusted(distance_code.len, u64::from(distance_code.bits));
                writer.write_bits_trusted(
                    record.distance_extra_bits,
                    u64::from(record.distance_extra),
                );
            }
        }

        Ok(())
    }
}

#[derive(Clone, Copy, Debug)]
struct Record {
    insert_start: u32,
    insert_len: u32,
    insert_extra: u32,
    copy_extra: u32,
    distance_extra: u32,
    command_symbol: u16,
    distance_symbol: u16,
    insert_extra_bits: u8,
    copy_extra_bits: u8,
    distance_extra_bits: u8,
}

pub(super) fn collect(
    input: &[u8],
    max_backward_distance: usize,
) -> Result<Box<Batch>, CompressError> {
    let mut batch = Box::new(Batch::with_capacity(input.len() / 32));
    if input.len() < 8 {
        batch.push(
            input,
            Token {
                insert_start: 0,
                insert_len: input.len(),
                copy_len: 0,
                distance: 0,
                use_last_distance: false,
            },
        )?;
        batch.ensure_distance_frequencies();
        return Ok(batch);
    }

    let mut table = vec![EMPTY; TABLE_SIZE];
    let mut pos = 0;
    let mut insert_start = 0;
    let mut last_distance = None;
    let mut word = read_u64_le(input, 0);

    while pos + 8 <= input.len() {
        let key = hash_word_q0(word, HASH_SHIFT) & TABLE_MASK;
        let previous = table[key];
        table[key] = pos as u32;

        let previous = last_distance
            .filter(|&distance| pos >= distance && is_match5(input, pos - distance, pos))
            .map(|distance| pos - distance)
            .or_else(|| {
                (previous != EMPTY)
                    .then_some(previous as usize)
                    .filter(|&previous| {
                        pos - previous <= max_backward_distance && is_match5(input, previous, pos)
                    })
            });

        if let Some(previous) = previous {
            let max_copy_len = (MAX_META_BLOCK_SIZE - (pos - insert_start)).min(input.len() - pos);
            let copy_len = match_len(input, previous, pos, max_copy_len);
            if copy_len >= 5 {
                let distance = pos - previous;
                let mut token = Token {
                    insert_start,
                    insert_len: pos - insert_start,
                    copy_len,
                    distance,
                    use_last_distance: false,
                };
                token.use_last_distance = distance
                    == last_distance.unwrap_or(INITIAL_LAST_DISTANCE)
                    && token_supports_last_distance(token);
                batch.push(input, token)?;
                store_match_range(input, &mut table, pos + 1, copy_len.saturating_sub(1));
                pos += copy_len;
                insert_start = pos;
                last_distance = Some(distance);
                if pos + 8 <= input.len() {
                    word = read_u64_le(input, pos);
                }
                continue;
            }
        }

        pos += 1;
        if pos + 8 <= input.len() {
            word = next_hash_word(word, input[pos + 7]);
        }
    }

    if insert_start < input.len() {
        batch.push(
            input,
            Token {
                insert_start,
                insert_len: input.len() - insert_start,
                copy_len: 0,
                distance: 0,
                use_last_distance: false,
            },
        )?;
    }

    batch.ensure_distance_frequencies();
    Ok(batch)
}

fn store_match_range(input: &[u8], table: &mut [u32], start: usize, copy_len: usize) {
    let end = start
        .saturating_add(copy_len)
        .min(input.len().saturating_sub(7));
    if start >= end {
        return;
    }

    let first = end.saturating_sub(3).max(start);
    let mut word = read_u64_le(input, first);
    for pos in first..end {
        let key = hash_word_q0(word, HASH_SHIFT) & TABLE_MASK;
        table[key] = pos as u32;
        let next = pos + 1;
        if next < end {
            word = next_hash_word(word, input[next + 7]);
        }
    }
}
