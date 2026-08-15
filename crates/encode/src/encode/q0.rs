use alloc::{vec, vec::Vec};

use burli_core::{BurliError, CompressError, bits::BitWriter};

use super::{
    COMMAND_ALPHABET_SIZE, DenseSymbolCode, INITIAL_LAST_DISTANCE, LITERAL_ALPHABET_SIZE,
    MAX_META_BLOCK_SIZE, PrefixCodeScratch, Token, append_pending_bits, command_symbol_for_insert,
    command_symbol_for_insert_copy, copy_length_code, distance_code, hash_word_q0,
    insert_length_code, is_match5, match_len, next_hash_word, read_u64_le,
    token_supports_last_distance, write_block_and_context_header,
    write_dense_prefix_code_array_from_frequencies_with_scratch_max_bits, write_meta_block_len,
};

const TABLE_SIZE: usize = 1 << 15;
const TABLE_MASK: usize = TABLE_SIZE - 1;
const HASH_SHIFT: usize = 49;
const POSITION_MASK: u32 = 0x00ff_ffff;
const NO_DISTANCE_SYMBOL: u32 = 127;
const COMMAND_SYMBOL_BITS: u32 = 10;
const DISTANCE_SYMBOL_BITS: u32 = 7;
const EXTRA_BITS_WIDTH: u32 = 5;
const COMMAND_SYMBOL_MASK: u32 = (1 << COMMAND_SYMBOL_BITS) - 1;
const DISTANCE_SYMBOL_MASK: u32 = (1 << DISTANCE_SYMBOL_BITS) - 1;
const EXTRA_BITS_MASK: u32 = (1 << EXTRA_BITS_WIDTH) - 1;
const DISTANCE_SYMBOL_SHIFT: u32 = COMMAND_SYMBOL_BITS;
const INSERT_EXTRA_BITS_SHIFT: u32 = DISTANCE_SYMBOL_SHIFT + DISTANCE_SYMBOL_BITS;
const COPY_EXTRA_BITS_SHIFT: u32 = INSERT_EXTRA_BITS_SHIFT + EXTRA_BITS_WIDTH;
const DISTANCE_EXTRA_BITS_SHIFT: u32 = COPY_EXTRA_BITS_SHIFT + EXTRA_BITS_WIDTH;

#[derive(Clone, Debug)]
pub(super) struct Batch {
    records: Vec<Record>,
    literal_frequencies: [usize; LITERAL_ALPHABET_SIZE],
    command_frequencies: [usize; COMMAND_ALPHABET_SIZE],
    distance_frequencies: [usize; 64],
    has_distance: bool,
    has_copy: bool,
}

#[derive(Clone, Debug, Default)]
pub(super) struct Workspace {
    table: HashTable,
    batch: Batch,
    prefix: PrefixCodeScratch,
}

#[derive(Clone, Debug, Default)]
struct HashTable {
    entries: Vec<u32>,
    generation: u32,
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

    fn reset(&mut self, capacity: usize) {
        self.records.clear();
        if self.records.capacity() < capacity {
            self.records.reserve(capacity - self.records.capacity());
        }
        self.literal_frequencies.fill(0);
        self.command_frequencies.fill(0);
        self.distance_frequencies.fill(0);
        self.has_distance = false;
        self.has_copy = false;
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
        let distance_symbol =
            distance.map_or(NO_DISTANCE_SYMBOL, |distance| u32::from(distance.symbol));
        self.records.push(Record {
            insert_start: token.insert_start as u32,
            insert_len: token.insert_len as u32,
            insert_extra: insert.extra as u32,
            copy_extra: copy.map_or(0, |copy| copy.extra as u32),
            distance_extra: distance.map_or(0, |distance| distance.extra as u32),
            meta: pack_record_meta(
                u32::from(command_symbol),
                distance_symbol,
                u32::from(insert.extra_bits),
                copy.map_or(0, |copy| u32::from(copy.extra_bits)),
                distance.map_or(0, |distance| u32::from(distance.extra_bits)),
            ),
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
        prefix: &mut PrefixCodeScratch,
    ) -> Result<(), CompressError> {
        if block_len == 0 || block_len > MAX_META_BLOCK_SIZE {
            return Err(BurliError::Format("invalid compressed Brotli block size"));
        }

        prefix.reserve_for(COMMAND_ALPHABET_SIZE, COMMAND_ALPHABET_SIZE);

        write_meta_block_len(writer, block_len)?;
        write_block_and_context_header(writer)?;
        let literal_code_map =
            write_dense_prefix_code_array_from_frequencies_with_scratch_max_bits(
                writer,
                &self.literal_frequencies,
                prefix,
                15,
            )?;
        let command_code_map =
            write_dense_prefix_code_array_from_frequencies_with_scratch_max_bits(
                writer,
                &self.command_frequencies,
                prefix,
                15,
            )?;
        let distance_code_map =
            write_dense_prefix_code_array_from_frequencies_with_scratch_max_bits(
                writer,
                &self.distance_frequencies,
                prefix,
                15,
            )?;

        let mut pending_bits = 0_u64;
        let mut pending_width = 0_u8;
        for record in &self.records {
            let meta = unpack_record_meta(record.meta);
            let command_code = command_code_map[meta.command_symbol as usize];
            debug_assert!(command_code.len != u8::MAX);
            append_pending_bits(
                writer,
                &mut pending_bits,
                &mut pending_width,
                command_code.len,
                u64::from(command_code.bits),
            );
            append_pending_bits(
                writer,
                &mut pending_bits,
                &mut pending_width,
                meta.insert_extra_bits,
                u64::from(record.insert_extra),
            );
            append_pending_bits(
                writer,
                &mut pending_bits,
                &mut pending_width,
                meta.copy_extra_bits,
                u64::from(record.copy_extra),
            );

            let insert_start = record.insert_start as usize;
            let insert_end = insert_start + record.insert_len as usize;
            append_literal_bits(
                writer,
                &mut pending_bits,
                &mut pending_width,
                &input[insert_start..insert_end],
                &literal_code_map,
            );

            if meta.distance_symbol != NO_DISTANCE_SYMBOL {
                let distance_code = distance_code_map[meta.distance_symbol as usize];
                debug_assert!(distance_code.len != u8::MAX);
                append_pending_bits(
                    writer,
                    &mut pending_bits,
                    &mut pending_width,
                    distance_code.len,
                    u64::from(distance_code.bits),
                );
                append_pending_bits(
                    writer,
                    &mut pending_bits,
                    &mut pending_width,
                    meta.distance_extra_bits,
                    u64::from(record.distance_extra),
                );
            }
        }
        if pending_width != 0 {
            writer.write_bits_trusted_nonzero_fits(pending_width, pending_bits);
        }

        Ok(())
    }
}

#[inline(always)]
fn append_literal_bits(
    writer: &mut BitWriter,
    pending_bits: &mut u64,
    pending_width: &mut u8,
    literals: &[u8],
    literal_code_map: &[DenseSymbolCode; LITERAL_ALPHABET_SIZE],
) {
    let mut pairs = literals.chunks_exact(2);
    for pair in &mut pairs {
        let first = literal_code_map[usize::from(pair[0])];
        let second = literal_code_map[usize::from(pair[1])];
        debug_assert!(first.len != u8::MAX);
        debug_assert!(second.len != u8::MAX);
        let width = first.len + second.len;
        let bits = u64::from(first.bits) | (u64::from(second.bits) << first.len);
        append_pending_bits(writer, pending_bits, pending_width, width, bits);
    }

    if let &[literal] = pairs.remainder() {
        let literal_code = literal_code_map[usize::from(literal)];
        debug_assert!(literal_code.len != u8::MAX);
        append_pending_bits(
            writer,
            pending_bits,
            pending_width,
            literal_code.len,
            u64::from(literal_code.bits),
        );
    }
}

impl Default for Batch {
    fn default() -> Self {
        Self::with_capacity(0)
    }
}

impl Workspace {
    fn write(
        &mut self,
        writer: &mut BitWriter,
        input: &[u8],
        block_len: usize,
    ) -> Result<(), CompressError> {
        self.batch.write(writer, input, block_len, &mut self.prefix)
    }

    fn reset(&mut self, record_capacity: usize) {
        self.table.reset();
        self.batch.reset(record_capacity);
    }
}

impl HashTable {
    fn reset(&mut self) {
        if self.entries.len() != TABLE_SIZE {
            self.entries = vec![0; TABLE_SIZE];
            self.generation = 1;
            return;
        }

        self.generation = (self.generation + 1) & 0xff;
        if self.generation == 0 {
            self.entries.fill(0);
            self.generation = 1;
        }
    }

    #[inline]
    fn load_and_store(&mut self, key: usize, position: usize) -> Option<usize> {
        let entry = &mut self.entries[key];
        let previous = (entry_generation(*entry) == self.generation)
            .then_some((*entry & POSITION_MASK) as usize);
        *entry = (self.generation << 24) | position as u32;
        previous
    }

    #[inline]
    fn store(&mut self, key: usize, position: usize) {
        self.entries[key] = (self.generation << 24) | position as u32;
    }
}

#[inline]
fn entry_generation(entry: u32) -> u32 {
    entry >> 24
}

#[derive(Clone, Copy, Debug)]
struct Record {
    insert_start: u32,
    insert_len: u32,
    insert_extra: u32,
    copy_extra: u32,
    distance_extra: u32,
    meta: u32,
}

#[derive(Clone, Copy, Debug)]
struct RecordMeta {
    command_symbol: u32,
    distance_symbol: u32,
    insert_extra_bits: u8,
    copy_extra_bits: u8,
    distance_extra_bits: u8,
}

fn pack_record_meta(
    command_symbol: u32,
    distance_symbol: u32,
    insert_extra_bits: u32,
    copy_extra_bits: u32,
    distance_extra_bits: u32,
) -> u32 {
    debug_assert!(command_symbol <= COMMAND_SYMBOL_MASK);
    debug_assert!(distance_symbol <= DISTANCE_SYMBOL_MASK);
    debug_assert!(insert_extra_bits <= EXTRA_BITS_MASK);
    debug_assert!(copy_extra_bits <= EXTRA_BITS_MASK);
    debug_assert!(distance_extra_bits <= EXTRA_BITS_MASK);

    command_symbol
        | (distance_symbol << DISTANCE_SYMBOL_SHIFT)
        | (insert_extra_bits << INSERT_EXTRA_BITS_SHIFT)
        | (copy_extra_bits << COPY_EXTRA_BITS_SHIFT)
        | (distance_extra_bits << DISTANCE_EXTRA_BITS_SHIFT)
}

fn unpack_record_meta(meta: u32) -> RecordMeta {
    RecordMeta {
        command_symbol: meta & COMMAND_SYMBOL_MASK,
        distance_symbol: (meta >> DISTANCE_SYMBOL_SHIFT) & DISTANCE_SYMBOL_MASK,
        insert_extra_bits: ((meta >> INSERT_EXTRA_BITS_SHIFT) & EXTRA_BITS_MASK) as u8,
        copy_extra_bits: ((meta >> COPY_EXTRA_BITS_SHIFT) & EXTRA_BITS_MASK) as u8,
        distance_extra_bits: ((meta >> DISTANCE_EXTRA_BITS_SHIFT) & EXTRA_BITS_MASK) as u8,
    }
}

pub(super) fn collect<'a>(
    input: &[u8],
    max_backward_distance: usize,
    workspace: &'a mut Workspace,
) -> Result<&'a Batch, CompressError> {
    workspace.collect(input, max_backward_distance)
}

pub(super) fn write(
    writer: &mut BitWriter,
    input: &[u8],
    block_len: usize,
    workspace: &mut Workspace,
) -> Result<(), CompressError> {
    workspace.write(writer, input, block_len)
}

impl Workspace {
    fn collect(
        &mut self,
        input: &[u8],
        max_backward_distance: usize,
    ) -> Result<&Batch, CompressError> {
        self.reset(input.len() / 32);
        if input.len() < 8 {
            self.batch.push(
                input,
                Token {
                    insert_start: 0,
                    insert_len: input.len(),
                    copy_len: 0,
                    copy_len_code: 0,
                    distance: 0,
                    distance_code: None,
                    use_last_distance: false,
                },
            )?;
            self.batch.ensure_distance_frequencies();
            return Ok(&self.batch);
        }

        let table = &mut self.table;
        let batch = &mut self.batch;
        let mut pos = 0;
        let mut insert_start = 0;
        let mut last_distance = None;
        let mut word = read_u64_le(input, 0);
        let mut skip = 32_usize;

        while pos + 8 <= input.len() {
            let key = hash_word_q0(word, HASH_SHIFT) & TABLE_MASK;
            let previous = table.load_and_store(key, pos);

            let previous = last_distance
                .filter(|&distance| pos >= distance && is_match5(input, pos - distance, pos))
                .map(|distance| pos - distance)
                .or_else(|| {
                    previous.filter(|&previous| {
                        pos - previous <= max_backward_distance && is_match5(input, previous, pos)
                    })
                });

            if let Some(previous) = previous {
                let max_copy_len =
                    (MAX_META_BLOCK_SIZE - (pos - insert_start)).min(input.len() - pos);
                let copy_len = match_len(input, previous, pos, max_copy_len);
                if copy_len >= 5 {
                    let distance = pos - previous;
                    let mut token = Token {
                        insert_start,
                        insert_len: pos - insert_start,
                        copy_len,
                        copy_len_code: 0,
                        distance,
                        distance_code: None,
                        use_last_distance: false,
                    };
                    token.use_last_distance = distance
                        == last_distance.unwrap_or(INITIAL_LAST_DISTANCE)
                        && token_supports_last_distance(token);
                    batch.push(input, token)?;
                    store_match_range(input, table, pos + 1, copy_len.saturating_sub(1));
                    pos += copy_len;
                    insert_start = pos;
                    last_distance = Some(distance);
                    skip = 32;
                    if pos + 8 <= input.len() {
                        word = read_u64_le(input, pos);
                    }
                    continue;
                }
            }

            let step = skip >> 5;
            skip += 1;
            pos += step;
            if pos + 8 <= input.len() {
                word = if step == 1 {
                    next_hash_word(word, input[pos + 7])
                } else {
                    read_u64_le(input, pos)
                };
            }
        }

        if insert_start < input.len() {
            batch.push(
                input,
                Token {
                    insert_start,
                    insert_len: input.len() - insert_start,
                    copy_len: 0,
                    copy_len_code: 0,
                    distance: 0,
                    distance_code: None,
                    use_last_distance: false,
                },
            )?;
        }

        batch.ensure_distance_frequencies();
        Ok(batch)
    }
}

fn store_match_range(input: &[u8], table: &mut HashTable, start: usize, copy_len: usize) {
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
        table.store(key, pos);
        let next = pos + 1;
        if next < end {
            word = next_hash_word(word, input[next + 7]);
        }
    }
}
