use alloc::vec::Vec;

use burli_core::{
    BurliError, CompressError,
    bits::{BitWriter, MAX_BITS_PER_OP},
};

use super::{
    COMMAND_ALPHABET_SIZE, DenseSymbolCode, LITERAL_ALPHABET_SIZE, MAX_META_BLOCK_SIZE,
    PrefixCodeScratch, append_pending_bits, match_len, read_u64_le, tune,
    write_block_and_context_header,
    write_fast_dense_prefix_code_array_from_frequencies_with_scratch, write_meta_block_len,
    write_q1_internal_balanced_command_static_distance_prefix_codes,
    write_q1_internal_command_prefix_codes, write_q1_internal_fast_command_prefix_codes,
};

const MAX_TABLE_SIZE: usize = 1 << 17;
const MIN_TABLE_SIZE: usize = 256;
const MAX_DISTANCE: usize = (1 << 18) - 16;
const INPUT_MARGIN_BYTES: usize = 16;
const HASH_MUL: u64 = 0x1e35_a7bd;
const HASH_MUL_32: u32 = 0x1e35_a7bd;
const NO_POSITION: u32 = u32::MAX;
const NO_POSITION_16: u16 = 0;
const NO_LAST_DISTANCE: usize = usize::MAX;
const INTERNAL_COMMAND_ALPHABET_SIZE: usize = 128;
const INTERNAL_DISTANCE_REUSE_CODE: usize = 64;
const INTERNAL_NUM_EXTRA_BITS: [u8; INTERNAL_COMMAND_ALPHABET_SIZE] = [
    0, 0, 0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 7, 8, 9, 10, 12, 14, 24, 0, 0, 0, 0, 0, 0,
    0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 7, 8, 9,
    10, 24, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7,
    7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13, 14, 14, 15, 15, 16, 16, 17, 17, 18, 18, 19, 19,
    20, 20, 21, 21, 22, 22, 23, 23, 24, 24,
];
const INTERNAL_INSERT_OFFSET: [usize; 24] = [
    0, 1, 2, 3, 4, 5, 6, 8, 10, 14, 18, 26, 34, 50, 66, 98, 130, 194, 322, 578, 1090, 2114, 6210,
    22594,
];

#[derive(Clone, Debug)]
pub(super) struct Batch {
    commands: Vec<u32>,
    literal_spans: Vec<LiteralSpan>,
    literal_frequencies: [usize; LITERAL_ALPHABET_SIZE],
    command_frequencies: [usize; INTERNAL_COMMAND_ALPHABET_SIZE],
    has_copy: bool,
}

#[derive(Clone, Copy, Debug)]
struct LiteralSpan {
    start: u32,
    len: u32,
}

#[derive(Clone, Debug, Default)]
pub(super) struct Workspace {
    table: Vec<u32>,
    small_table: Vec<u16>,
    batch: Batch,
    prefix: PrefixCodeScratch,
}

impl Batch {
    fn with_capacity(capacity: usize) -> Self {
        Self {
            commands: Vec::with_capacity(capacity),
            literal_spans: Vec::with_capacity(capacity),
            literal_frequencies: [0; LITERAL_ALPHABET_SIZE],
            command_frequencies: [0; INTERNAL_COMMAND_ALPHABET_SIZE],
            has_copy: false,
        }
    }

    fn reset(&mut self, input_len: usize) {
        self.commands.clear();
        let command_capacity = input_len / 4 + 8;
        if self.commands.capacity() < command_capacity {
            self.commands
                .reserve(command_capacity - self.commands.capacity());
        }
        self.literal_spans.clear();
        if self.literal_spans.capacity() < command_capacity {
            self.literal_spans
                .reserve(command_capacity - self.literal_spans.capacity());
        }
        self.literal_frequencies.fill(0);
        self.command_frequencies.fill(0);
        self.has_copy = false;
    }

    pub(super) const fn has_copy(&self) -> bool {
        self.has_copy
    }

    #[inline(always)]
    fn push_literals(&mut self, input: &[u8], insert_start: usize, insert_len: usize) {
        if insert_len == 0 {
            return;
        }
        self.record_literals(input, insert_start, insert_len);
        self.emit_insert_len(insert_len);
    }

    #[inline(always)]
    fn record_literals(&mut self, input: &[u8], insert_start: usize, insert_len: usize) {
        let insert_end = insert_start + insert_len;
        debug_assert!(insert_end <= input.len());
        let inserted = &input[insert_start..insert_end];
        for &literal in inserted {
            self.literal_frequencies[usize::from(literal)] += 1;
        }
        self.literal_spans.push(LiteralSpan {
            start: insert_start as u32,
            len: insert_len as u32,
        });
    }

    #[inline(always)]
    fn emit_command(&mut self, code: usize, extra: usize) {
        debug_assert!(code < INTERNAL_COMMAND_ALPHABET_SIZE);
        debug_assert!(extra <= (u32::MAX as usize >> 8));
        self.command_frequencies[code] += 1;
        self.commands.push((code as u32) | ((extra as u32) << 8));
    }

    #[inline(always)]
    fn push_copy(
        &mut self,
        input: &[u8],
        insert_start: usize,
        insert_len: usize,
        copy_len: usize,
        match_distance: usize,
        last_distance: usize,
    ) {
        if insert_len != 0 {
            self.record_literals(input, insert_start, insert_len);
        }
        self.has_copy = true;

        if insert_len == 0 {
            self.emit_copy_len(copy_len);
            self.emit_distance(match_distance);
        } else {
            self.emit_insert_len(insert_len);
            if match_distance == last_distance {
                self.emit_command(INTERNAL_DISTANCE_REUSE_CODE, 0);
            } else {
                self.emit_distance(match_distance);
            }
            self.emit_copy_len_last_distance(copy_len);
        }
    }

    #[inline(always)]
    fn emit_insert_len(&mut self, insert_len: usize) {
        if insert_len < 6 {
            self.emit_command(insert_len, 0);
            return;
        }
        if insert_len < 130 {
            let tail = insert_len - 2;
            let nbits = log2_floor(tail) - 1;
            let prefix = tail >> nbits;
            let code = (nbits << 1) + prefix + 2;
            let extra = tail - (prefix << nbits);
            self.emit_command(code, extra);
            return;
        }
        if insert_len < 2114 {
            let tail = insert_len - 66;
            let nbits = log2_floor(tail);
            let code = nbits + 10;
            let extra = tail - (1_usize << nbits);
            self.emit_command(code, extra);
            return;
        }
        if insert_len < 6210 {
            self.emit_command(21, insert_len - 2114);
            return;
        }
        if insert_len < 22594 {
            self.emit_command(22, insert_len - 6210);
            return;
        }
        self.emit_command(23, insert_len - 22594);
    }

    #[inline(always)]
    fn emit_copy_len(&mut self, copy_len: usize) {
        if copy_len < 10 {
            self.emit_command(copy_len + 38, 0);
            return;
        }
        if copy_len < 134 {
            let tail = copy_len - 6;
            let nbits = log2_floor(tail) - 1;
            let prefix = tail >> nbits;
            let code = (nbits << 1) + prefix + 44;
            let extra = tail - (prefix << nbits);
            self.emit_command(code, extra);
            return;
        }
        if copy_len < 2118 {
            let tail = copy_len - 70;
            let nbits = log2_floor(tail);
            let code = nbits + 52;
            let extra = tail - (1_usize << nbits);
            self.emit_command(code, extra);
            return;
        }
        self.emit_command(63, copy_len - 2118);
    }

    #[inline(always)]
    fn emit_copy_len_last_distance(&mut self, copy_len: usize) {
        if copy_len < 12 {
            self.emit_command(copy_len + 20, 0);
            return;
        }
        if copy_len < 72 {
            let tail = copy_len - 8;
            let nbits = log2_floor(tail) - 1;
            let prefix = tail >> nbits;
            let code = (nbits << 1) + prefix + 28;
            let extra = tail - (prefix << nbits);
            self.emit_command(code, extra);
            return;
        }
        if copy_len < 136 {
            let tail = copy_len - 8;
            self.emit_command((tail >> 5) + 54, tail & 31);
            self.emit_command(INTERNAL_DISTANCE_REUSE_CODE, 0);
            return;
        }
        if copy_len < 2120 {
            let tail = copy_len - 72;
            let nbits = log2_floor(tail);
            let code = nbits + 52;
            let extra = tail - (1_usize << nbits);
            self.emit_command(code, extra);
            self.emit_command(INTERNAL_DISTANCE_REUSE_CODE, 0);
            return;
        }
        self.emit_command(63, copy_len - 2120);
        self.emit_command(INTERNAL_DISTANCE_REUSE_CODE, 0);
    }

    #[inline(always)]
    fn emit_distance(&mut self, distance: usize) {
        let d = distance + 3;
        let nbits = log2_floor(d) - 1;
        let prefix = (d >> nbits) & 1;
        let offset = (2 + prefix) << nbits;
        let code = 2 * (nbits - 1) + prefix + 80;
        self.emit_command(code, d - offset);
    }

    pub(super) fn write(
        &self,
        writer: &mut BitWriter,
        input: &[u8],
        block_len: usize,
        prefix: &mut PrefixCodeScratch,
        fast_literal_prefix: bool,
    ) -> Result<(), CompressError> {
        if block_len == 0 || block_len > MAX_META_BLOCK_SIZE {
            return Err(BurliError::Format("invalid compressed Brotli block size"));
        }

        write_meta_block_len(writer, block_len)?;
        write_block_and_context_header(writer)?;
        let literal_code_map = self.write_literal_prefix(writer, prefix, fast_literal_prefix)?;
        let mut command_frequencies = self.command_frequencies;
        command_frequencies[1] += 1;
        command_frequencies[2] += 1;
        command_frequencies[64] += 1;
        command_frequencies[84] += 1;
        let command_code_map =
            write_q1_internal_command_prefix_codes(writer, &command_frequencies, prefix)?;

        write_batch_body::<false>(writer, input, self, &literal_code_map, &command_code_map)
    }

    pub(super) fn write_q0(
        &self,
        writer: &mut BitWriter,
        input: &[u8],
        block_len: usize,
        prefix: &mut PrefixCodeScratch,
        fast_literal_prefix: bool,
    ) -> Result<(), CompressError> {
        if block_len == 0 || block_len > MAX_META_BLOCK_SIZE {
            return Err(BurliError::Format("invalid compressed Brotli block size"));
        }

        write_meta_block_len(writer, block_len)?;
        write_block_and_context_header(writer)?;
        let literal_code_map = self.write_literal_prefix(writer, prefix, fast_literal_prefix)?;
        let mut command_frequencies = self.command_frequencies;
        add_q0_command_guards(&mut command_frequencies);
        let command_code_map =
            write_q1_internal_command_prefix_codes(writer, &command_frequencies, prefix)?;

        write_q0_batch_body::<false>(writer, input, self, &literal_code_map, &command_code_map);
        Ok(())
    }

    pub(super) fn write_q0_balanced_literal_command_prefixes(
        &self,
        writer: &mut BitWriter,
        input: &[u8],
        block_len: usize,
        prefix: &mut PrefixCodeScratch,
    ) -> Result<(), CompressError> {
        if block_len == 0 || block_len > MAX_META_BLOCK_SIZE {
            return Err(BurliError::Format("invalid compressed Brotli block size"));
        }

        write_meta_block_len(writer, block_len)?;
        write_block_and_context_header(writer)?;
        let literal_code_map = self.write_balanced_literal_prefix(writer, prefix)?;
        let mut command_frequencies = self.command_frequencies;
        add_q0_command_guards(&mut command_frequencies);
        let command_code_map = write_q1_internal_balanced_command_static_distance_prefix_codes(
            writer,
            &command_frequencies,
            prefix,
        )?;

        write_q0_batch_body::<false>(writer, input, self, &literal_code_map, &command_code_map);
        Ok(())
    }

    pub(super) fn write_q0_balanced_command_prefixes(
        &self,
        writer: &mut BitWriter,
        input: &[u8],
        block_len: usize,
        prefix: &mut PrefixCodeScratch,
        fast_literal_prefix: bool,
    ) -> Result<(), CompressError> {
        if block_len == 0 || block_len > MAX_META_BLOCK_SIZE {
            return Err(BurliError::Format("invalid compressed Brotli block size"));
        }

        write_meta_block_len(writer, block_len)?;
        write_block_and_context_header(writer)?;
        let literal_code_map = self.write_literal_prefix(writer, prefix, fast_literal_prefix)?;
        let mut command_frequencies = self.command_frequencies;
        add_q0_command_guards(&mut command_frequencies);
        let command_code_map = write_q1_internal_balanced_command_static_distance_prefix_codes(
            writer,
            &command_frequencies,
            prefix,
        )?;

        write_q0_batch_body::<false>(writer, input, self, &literal_code_map, &command_code_map);
        Ok(())
    }

    pub(super) fn write_q0_fast_command_prefixes(
        &self,
        writer: &mut BitWriter,
        input: &[u8],
        block_len: usize,
        prefix: &mut PrefixCodeScratch,
        fast_literal_prefix: bool,
    ) -> Result<(), CompressError> {
        if block_len == 0 || block_len > MAX_META_BLOCK_SIZE {
            return Err(BurliError::Format("invalid compressed Brotli block size"));
        }

        write_meta_block_len(writer, block_len)?;
        write_block_and_context_header(writer)?;
        let literal_code_map = self.write_literal_prefix(writer, prefix, fast_literal_prefix)?;
        let mut command_frequencies = self.command_frequencies;
        add_q0_command_guards(&mut command_frequencies);
        let command_code_map =
            write_q1_internal_fast_command_prefix_codes(writer, &command_frequencies, prefix)?;

        write_q0_batch_body::<false>(writer, input, self, &literal_code_map, &command_code_map);
        Ok(())
    }

    pub(super) fn write_q0_packed_literal_body(
        &self,
        writer: &mut BitWriter,
        input: &[u8],
        block_len: usize,
        prefix: &mut PrefixCodeScratch,
        fast_literal_prefix: bool,
    ) -> Result<(), CompressError> {
        if block_len == 0 || block_len > MAX_META_BLOCK_SIZE {
            return Err(BurliError::Format("invalid compressed Brotli block size"));
        }

        write_meta_block_len(writer, block_len)?;
        write_block_and_context_header(writer)?;
        let literal_code_map = self.write_literal_prefix(writer, prefix, fast_literal_prefix)?;
        let mut command_frequencies = self.command_frequencies;
        add_q0_command_guards(&mut command_frequencies);
        let command_code_map =
            write_q1_internal_command_prefix_codes(writer, &command_frequencies, prefix)?;

        write_q0_batch_body::<true>(writer, input, self, &literal_code_map, &command_code_map);
        Ok(())
    }

    fn write_literal_prefix(
        &self,
        writer: &mut BitWriter,
        prefix: &mut PrefixCodeScratch,
        fast_literal_prefix: bool,
    ) -> Result<[DenseSymbolCode; LITERAL_ALPHABET_SIZE], CompressError> {
        prefix.reserve_for(LITERAL_ALPHABET_SIZE, COMMAND_ALPHABET_SIZE);
        if fast_literal_prefix {
            return write_fast_dense_prefix_code_array_from_frequencies_with_scratch(
                writer,
                &self.literal_frequencies,
                prefix,
            );
        }
        super::write_dense_prefix_code_array_from_frequencies_with_scratch_max_bits(
            writer,
            &self.literal_frequencies,
            prefix,
            15,
        )
    }

    fn write_balanced_literal_prefix(
        &self,
        writer: &mut BitWriter,
        prefix: &mut PrefixCodeScratch,
    ) -> Result<[DenseSymbolCode; LITERAL_ALPHABET_SIZE], CompressError> {
        prefix.reserve_for(LITERAL_ALPHABET_SIZE, COMMAND_ALPHABET_SIZE);
        super::write_balanced_fast_dense_prefix_code_array_from_frequencies_with_scratch(
            writer,
            &self.literal_frequencies,
            prefix,
        )
    }
}

#[inline(always)]
fn add_q0_command_guards(command_frequencies: &mut [usize; INTERNAL_COMMAND_ALPHABET_SIZE]) {
    command_frequencies[1] += 1;
    command_frequencies[2] += 1;
    command_frequencies[64] += 1;
    command_frequencies[84] += 1;
}

#[inline(never)]
fn write_batch_body<const PACK_LITERALS: bool>(
    writer: &mut BitWriter,
    input: &[u8],
    batch: &Batch,
    literal_code_map: &[DenseSymbolCode; LITERAL_ALPHABET_SIZE],
    command_code_map: &[DenseSymbolCode; INTERNAL_COMMAND_ALPHABET_SIZE],
) -> Result<(), CompressError> {
    let mut literal_span_index = 0_usize;
    let mut pending_bits = 0_u64;
    let mut pending_width = 0_u8;
    for &command in &batch.commands {
        let code = (command & 0xff) as usize;
        let extra = (command >> 8) as usize;
        debug_assert!(code < INTERNAL_COMMAND_ALPHABET_SIZE);
        let command_code = command_code_map[code];
        let extra_bits = INTERNAL_NUM_EXTRA_BITS[code];
        debug_assert!(command_code.len != u8::MAX);
        debug_assert!(extra_bits == 0 || extra < (1_usize << extra_bits));
        let command_width = command_code.len + extra_bits;
        let command_bits = u64::from(command_code.bits) | ((extra as u64) << command_code.len);
        append_pending_bits(
            writer,
            &mut pending_bits,
            &mut pending_width,
            command_width,
            command_bits,
        );

        if code < INTERNAL_INSERT_OFFSET.len() {
            let insert_len = INTERNAL_INSERT_OFFSET[code] + extra;
            debug_assert!(literal_span_index < batch.literal_spans.len());
            let span = batch.literal_spans[literal_span_index];
            if span.len as usize != insert_len {
                return Err(BurliError::Format("Brotli q1 literal span mismatch"));
            }
            let start = span.start as usize;
            let end = start + span.len as usize;
            debug_assert!(end <= input.len());
            append_literal_span_bits::<PACK_LITERALS>(
                writer,
                &mut pending_bits,
                &mut pending_width,
                &input[start..end],
                literal_code_map,
            );
            literal_span_index += 1;
        }
    }
    if pending_width != 0 {
        writer.write_bits_trusted_nonzero_fits(pending_width, pending_bits);
    }

    if literal_span_index != batch.literal_spans.len() {
        return Err(BurliError::Format("Brotli q1 literal span mismatch"));
    }

    Ok(())
}

#[inline(never)]
fn write_q0_batch_body<const PACK_LITERALS: bool>(
    writer: &mut BitWriter,
    input: &[u8],
    batch: &Batch,
    literal_code_map: &[DenseSymbolCode; LITERAL_ALPHABET_SIZE],
    command_code_map: &[DenseSymbolCode; INTERNAL_COMMAND_ALPHABET_SIZE],
) {
    let mut literal_span_index = 0_usize;
    let mut pending_bits = 0_u64;
    let mut pending_width = 0_u8;
    for &command in &batch.commands {
        let code = (command & 0xff) as usize;
        let extra = (command >> 8) as usize;
        debug_assert!(code < INTERNAL_COMMAND_ALPHABET_SIZE);
        let command_code = command_code_map[code];
        let extra_bits = INTERNAL_NUM_EXTRA_BITS[code];
        debug_assert!(command_code.len != u8::MAX);
        debug_assert!(extra_bits == 0 || extra < (1_usize << extra_bits));
        append_pending_bits(
            writer,
            &mut pending_bits,
            &mut pending_width,
            command_code.len + extra_bits,
            u64::from(command_code.bits) | ((extra as u64) << command_code.len),
        );

        if code < INTERNAL_INSERT_OFFSET.len() {
            let span = batch.literal_spans[literal_span_index];
            let start = span.start as usize;
            let end = start + span.len as usize;
            debug_assert_eq!(span.len as usize, INTERNAL_INSERT_OFFSET[code] + extra);
            debug_assert!(end <= input.len());
            append_literal_span_bits::<PACK_LITERALS>(
                writer,
                &mut pending_bits,
                &mut pending_width,
                &input[start..end],
                literal_code_map,
            );
            literal_span_index += 1;
        }
    }
    debug_assert_eq!(literal_span_index, batch.literal_spans.len());
    if pending_width != 0 {
        writer.write_bits_trusted_nonzero_fits(pending_width, pending_bits);
    }
}

#[inline(always)]
fn append_literal_span_bits<const PACK_LITERALS: bool>(
    writer: &mut BitWriter,
    pending_bits: &mut u64,
    pending_width: &mut u8,
    literals: &[u8],
    literal_code_map: &[DenseSymbolCode; LITERAL_ALPHABET_SIZE],
) {
    if PACK_LITERALS {
        return append_literal_span_bits_packed(
            writer,
            pending_bits,
            pending_width,
            literals,
            literal_code_map,
        );
    }

    append_literal_span_bits_paired(
        writer,
        pending_bits,
        pending_width,
        literals,
        literal_code_map,
    );
}

#[inline(always)]
fn append_literal_span_bits_paired(
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
        append_literal_pair_bits(
            writer,
            pending_bits,
            pending_width,
            first,
            second,
            first.len + second.len,
        );
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

#[inline(always)]
fn append_literal_span_bits_packed(
    writer: &mut BitWriter,
    pending_bits: &mut u64,
    pending_width: &mut u8,
    literals: &[u8],
    literal_code_map: &[DenseSymbolCode; LITERAL_ALPHABET_SIZE],
) {
    let mut chunks = literals.chunks_exact(4);
    for chunk in &mut chunks {
        let first = literal_code_map[usize::from(chunk[0])];
        let second = literal_code_map[usize::from(chunk[1])];
        let third = literal_code_map[usize::from(chunk[2])];
        let fourth = literal_code_map[usize::from(chunk[3])];
        debug_assert!(first.len != u8::MAX);
        debug_assert!(second.len != u8::MAX);
        debug_assert!(third.len != u8::MAX);
        debug_assert!(fourth.len != u8::MAX);

        let first_width = first.len + second.len;
        let second_width = third.len + fourth.len;
        let width = first_width + second_width;
        if width <= MAX_BITS_PER_OP {
            let bits = u64::from(first.bits)
                | (u64::from(second.bits) << first.len)
                | (u64::from(third.bits) << first_width)
                | (u64::from(fourth.bits) << (first_width + third.len));
            append_pending_bits(writer, pending_bits, pending_width, width, bits);
        } else {
            append_literal_pair_bits(
                writer,
                pending_bits,
                pending_width,
                first,
                second,
                first_width,
            );
            append_literal_pair_bits(
                writer,
                pending_bits,
                pending_width,
                third,
                fourth,
                second_width,
            );
        }
    }

    append_literal_span_bits_paired(
        writer,
        pending_bits,
        pending_width,
        chunks.remainder(),
        literal_code_map,
    );
}

#[inline(always)]
fn append_literal_pair_bits(
    writer: &mut BitWriter,
    pending_bits: &mut u64,
    pending_width: &mut u8,
    first: DenseSymbolCode,
    second: DenseSymbolCode,
    width: u8,
) {
    let bits = u64::from(first.bits) | (u64::from(second.bits) << first.len);
    append_pending_bits(writer, pending_bits, pending_width, width, bits);
}

#[inline(always)]
fn log2_floor(value: usize) -> usize {
    debug_assert!(value != 0);
    usize::BITS as usize - 1 - value.leading_zeros() as usize
}

impl Default for Batch {
    fn default() -> Self {
        Self::with_capacity(0)
    }
}

pub(super) fn collect<'a>(
    input: &[u8],
    max_backward_distance: usize,
    workspace: &'a mut Workspace,
) -> Result<&'a Batch, CompressError> {
    workspace.collect(input, max_backward_distance)
}

pub(super) fn collect_with_64k_medium_skip<'a>(
    input: &[u8],
    max_backward_distance: usize,
    workspace: &'a mut Workspace,
) -> &'a Batch {
    workspace.collect_with_64k_medium_skip(input, max_backward_distance)
}

pub(super) fn collect_with_64k_fast_skip<'a>(
    input: &[u8],
    max_backward_distance: usize,
    workspace: &'a mut Workspace,
) -> &'a Batch {
    workspace.collect_with_64k_fast_skip(input, max_backward_distance)
}

pub(super) fn collect_q0_2k_fast_no_last<'a>(
    input: &[u8],
    max_backward_distance: usize,
    workspace: &'a mut Workspace,
) -> &'a Batch {
    workspace.collect_q0_2k_fast_no_last(input, max_backward_distance)
}

pub(super) fn collect_q0_4k_no_last<'a>(
    input: &[u8],
    max_backward_distance: usize,
    workspace: &'a mut Workspace,
) -> &'a Batch {
    workspace.collect_q0_4k_no_last(input, max_backward_distance)
}

pub(super) fn collect_q0_8k_default<'a>(
    input: &[u8],
    max_backward_distance: usize,
    workspace: &'a mut Workspace,
) -> &'a Batch {
    workspace.collect_q0_8k_default(input, max_backward_distance)
}

pub(super) fn collect_q0_16k_medium_no_last<'a>(
    input: &[u8],
    max_backward_distance: usize,
    workspace: &'a mut Workspace,
) -> &'a Batch {
    workspace.collect_q0_16k_medium_no_last(input, max_backward_distance)
}

pub(super) fn collect_q0_32k_medium<'a>(
    input: &[u8],
    max_backward_distance: usize,
    workspace: &'a mut Workspace,
) -> &'a Batch {
    workspace.collect_q0_32k_medium(input, max_backward_distance)
}

pub(super) fn collect_with_64k_sparse_stride<'a>(
    input: &[u8],
    max_backward_distance: usize,
    workspace: &'a mut Workspace,
) -> &'a Batch {
    workspace.collect_with_64k_sparse_stride(input, max_backward_distance)
}

pub(super) fn collect_with_32k_dense_skip<'a>(
    input: &[u8],
    max_backward_distance: usize,
    workspace: &'a mut Workspace,
) -> &'a Batch {
    workspace.collect_with_32k_dense_skip(input, max_backward_distance)
}

pub(super) fn collect_with_32k_u16_skip<'a>(
    input: &[u8],
    max_backward_distance: usize,
    workspace: &'a mut Workspace,
) -> &'a Batch {
    workspace.collect_with_32k_u16_skip(input, max_backward_distance)
}

pub(super) fn collect_with_32k_faster_skip<'a>(
    input: &[u8],
    max_backward_distance: usize,
    workspace: &'a mut Workspace,
) -> &'a Batch {
    workspace.collect_with_32k_faster_skip(input, max_backward_distance)
}

pub(super) fn write(
    writer: &mut BitWriter,
    input: &[u8],
    block_len: usize,
    workspace: &mut Workspace,
    fast_literal_prefix: bool,
) -> Result<(), CompressError> {
    workspace.write(writer, input, block_len, fast_literal_prefix)
}

pub(super) fn write_q0(
    writer: &mut BitWriter,
    input: &[u8],
    block_len: usize,
    workspace: &mut Workspace,
    fast_literal_prefix: bool,
) -> Result<(), CompressError> {
    workspace.write_q0(writer, input, block_len, fast_literal_prefix)
}

pub(super) fn write_q0_balanced_literal_command_prefixes(
    writer: &mut BitWriter,
    input: &[u8],
    block_len: usize,
    workspace: &mut Workspace,
) -> Result<(), CompressError> {
    workspace.write_q0_balanced_literal_command_prefixes(writer, input, block_len)
}

pub(super) fn write_q0_balanced_command_prefixes(
    writer: &mut BitWriter,
    input: &[u8],
    block_len: usize,
    workspace: &mut Workspace,
    fast_literal_prefix: bool,
) -> Result<(), CompressError> {
    workspace.write_q0_balanced_command_prefixes(writer, input, block_len, fast_literal_prefix)
}

pub(super) fn write_q0_fast_command_prefixes(
    writer: &mut BitWriter,
    input: &[u8],
    block_len: usize,
    workspace: &mut Workspace,
    fast_literal_prefix: bool,
) -> Result<(), CompressError> {
    workspace.write_q0_fast_command_prefixes(writer, input, block_len, fast_literal_prefix)
}

pub(super) fn write_q0_packed_literal_body(
    writer: &mut BitWriter,
    input: &[u8],
    block_len: usize,
    workspace: &mut Workspace,
    fast_literal_prefix: bool,
) -> Result<(), CompressError> {
    workspace.write_q0_packed_literal_body(writer, input, block_len, fast_literal_prefix)
}

impl Workspace {
    fn write(
        &mut self,
        writer: &mut BitWriter,
        input: &[u8],
        block_len: usize,
        fast_literal_prefix: bool,
    ) -> Result<(), CompressError> {
        self.batch.write(
            writer,
            input,
            block_len,
            &mut self.prefix,
            fast_literal_prefix,
        )
    }

    fn write_q0(
        &mut self,
        writer: &mut BitWriter,
        input: &[u8],
        block_len: usize,
        fast_literal_prefix: bool,
    ) -> Result<(), CompressError> {
        self.batch.write_q0(
            writer,
            input,
            block_len,
            &mut self.prefix,
            fast_literal_prefix,
        )
    }

    fn write_q0_balanced_literal_command_prefixes(
        &mut self,
        writer: &mut BitWriter,
        input: &[u8],
        block_len: usize,
    ) -> Result<(), CompressError> {
        self.batch.write_q0_balanced_literal_command_prefixes(
            writer,
            input,
            block_len,
            &mut self.prefix,
        )
    }

    fn write_q0_balanced_command_prefixes(
        &mut self,
        writer: &mut BitWriter,
        input: &[u8],
        block_len: usize,
        fast_literal_prefix: bool,
    ) -> Result<(), CompressError> {
        self.batch.write_q0_balanced_command_prefixes(
            writer,
            input,
            block_len,
            &mut self.prefix,
            fast_literal_prefix,
        )
    }

    fn write_q0_fast_command_prefixes(
        &mut self,
        writer: &mut BitWriter,
        input: &[u8],
        block_len: usize,
        fast_literal_prefix: bool,
    ) -> Result<(), CompressError> {
        self.batch.write_q0_fast_command_prefixes(
            writer,
            input,
            block_len,
            &mut self.prefix,
            fast_literal_prefix,
        )
    }

    fn write_q0_packed_literal_body(
        &mut self,
        writer: &mut BitWriter,
        input: &[u8],
        block_len: usize,
        fast_literal_prefix: bool,
    ) -> Result<(), CompressError> {
        self.batch.write_q0_packed_literal_body(
            writer,
            input,
            block_len,
            &mut self.prefix,
            fast_literal_prefix,
        )
    }

    fn collect(
        &mut self,
        input: &[u8],
        max_backward_distance: usize,
    ) -> Result<&Batch, CompressError> {
        self.reset(input.len());

        if input.len() < INPUT_MARGIN_BYTES {
            self.push_literals(input, 0, input.len());
            return Ok(&self.batch);
        }

        let table_size = table_size(input.len());
        if self.collect_stack_u16_for_size::<{ tune::Q1_DEFAULT_U16_SKIP_START }>(
            input,
            max_backward_distance,
            table_size,
        ) {
            return Ok(&self.batch);
        }

        let table_bits = table_size.trailing_zeros() as usize;
        if table_size <= 32768 {
            if self.small_table.len() != table_size {
                self.small_table.resize(table_size, NO_POSITION_16);
            } else {
                self.small_table.fill(NO_POSITION_16);
            }
            collect_with_u16_table(
                &mut self.batch,
                input,
                max_backward_distance,
                &mut self.small_table,
                table_bits,
            )?;
            return Ok(&self.batch);
        }

        if self.table.len() != table_size {
            self.table.resize(table_size, 0);
        } else {
            self.table.fill(0);
        }

        let min_match = if table_bits <= 15 { 4 } else { 6 };
        match table_bits {
            16 => {
                collect_with_u32_table_m6::<16, { tune::Q1_DEFAULT_U32_SKIP_START }, true, true>(
                    &mut self.batch,
                    input,
                    max_backward_distance,
                    &mut self.table,
                );
                return Ok(&self.batch);
            }
            17 => {
                collect_with_u32_table_m6::<17, { tune::Q1_DEFAULT_U32_SKIP_START }, true, true>(
                    &mut self.batch,
                    input,
                    max_backward_distance,
                    &mut self.table,
                );
                return Ok(&self.batch);
            }
            _ => {}
        }
        let max_distance = max_backward_distance.min(MAX_DISTANCE);
        let len_limit = input
            .len()
            .saturating_sub(min_match)
            .min(input.len().saturating_sub(INPUT_MARGIN_BYTES));
        if len_limit <= 1 {
            self.push_literals(input, 0, input.len());
            return Ok(&self.batch);
        }

        let mut pos = 1;
        let mut insert_start = 0;
        let mut next_hash = hash(input, pos, table_bits, min_match);
        let mut last_distance = NO_LAST_DISTANCE;

        while let Some(mut candidate) = self.scan_to_match(
            input,
            &mut pos,
            &mut next_hash,
            len_limit,
            max_distance,
            table_bits,
            min_match,
            last_distance,
        ) {
            loop {
                let distance = pos - candidate;
                let max_copy_len =
                    (MAX_META_BLOCK_SIZE - (pos - insert_start)).min(input.len() - pos);
                if max_copy_len < min_match {
                    break;
                }
                let copy_len = min_match
                    + match_len(
                        input,
                        candidate + min_match,
                        pos + min_match,
                        max_copy_len - min_match,
                    );

                self.batch.push_copy(
                    input,
                    insert_start,
                    pos - insert_start,
                    copy_len,
                    distance,
                    last_distance,
                );
                pos += copy_len;
                insert_start = pos;
                last_distance = distance;
                self.store_tail_hashes(input, pos, table_bits, min_match);

                if pos > len_limit {
                    break;
                }

                let key = hash(input, pos, table_bits, min_match);
                candidate = self.table[key] as usize;
                self.table[key] = pos as u32;
                if candidate == NO_POSITION as usize
                    || candidate >= pos
                    || pos - candidate > max_distance
                    || !is_match(input, candidate, pos, min_match)
                {
                    break;
                }
            }

            pos += 1;
            if pos > len_limit {
                break;
            }
            next_hash = hash(input, pos, table_bits, min_match);
        }

        if insert_start < input.len() {
            self.push_literals(input, insert_start, input.len() - insert_start);
        }

        Ok(&self.batch)
    }

    fn collect_q0_2k_fast_no_last(&mut self, input: &[u8], max_backward_distance: usize) -> &Batch {
        self.reset(input.len());
        collect_with_stack_u16_table::<11, 2048, { tune::Q1_DEFAULT_U16_SKIP_START }, false>(
            &mut self.batch,
            input,
            max_backward_distance,
        );
        &self.batch
    }

    fn collect_q0_4k_no_last(&mut self, input: &[u8], max_backward_distance: usize) -> &Batch {
        self.reset(input.len());
        collect_with_stack_u16_table::<12, 4096, { tune::Q1_DEFAULT_U16_SKIP_START }, false>(
            &mut self.batch,
            input,
            max_backward_distance,
        );
        &self.batch
    }

    fn collect_q0_8k_default(&mut self, input: &[u8], max_backward_distance: usize) -> &Batch {
        self.reset(input.len());
        collect_with_stack_u16_table::<13, 8192, { tune::Q1_DEFAULT_U16_SKIP_START }, true>(
            &mut self.batch,
            input,
            max_backward_distance,
        );
        &self.batch
    }

    fn collect_q0_16k_medium_no_last(
        &mut self,
        input: &[u8],
        max_backward_distance: usize,
    ) -> &Batch {
        self.reset(input.len());
        collect_with_stack_u16_table::<14, 16384, { tune::Q1_MEDIUM_U16_SKIP_START }, false>(
            &mut self.batch,
            input,
            max_backward_distance,
        );
        &self.batch
    }

    fn collect_q0_32k_medium(&mut self, input: &[u8], max_backward_distance: usize) -> &Batch {
        self.reset(input.len());
        collect_with_stack_u16_table::<15, 32768, { tune::Q1_MEDIUM_U16_SKIP_START }, true>(
            &mut self.batch,
            input,
            max_backward_distance,
        );
        &self.batch
    }

    fn collect_stack_u16_for_size<const SKIP_START: usize>(
        &mut self,
        input: &[u8],
        max_backward_distance: usize,
        table_size: usize,
    ) -> bool {
        match table_size {
            256 => collect_with_stack_u16_table::<8, 256, SKIP_START, true>(
                &mut self.batch,
                input,
                max_backward_distance,
            ),
            512 => collect_with_stack_u16_table::<9, 512, SKIP_START, true>(
                &mut self.batch,
                input,
                max_backward_distance,
            ),
            1024 => collect_with_stack_u16_table::<10, 1024, SKIP_START, true>(
                &mut self.batch,
                input,
                max_backward_distance,
            ),
            2048 => collect_with_stack_u16_table::<11, 2048, SKIP_START, true>(
                &mut self.batch,
                input,
                max_backward_distance,
            ),
            4096 => collect_with_stack_u16_table::<12, 4096, SKIP_START, true>(
                &mut self.batch,
                input,
                max_backward_distance,
            ),
            8192 => collect_with_stack_u16_table::<13, 8192, SKIP_START, true>(
                &mut self.batch,
                input,
                max_backward_distance,
            ),
            16384 => collect_with_stack_u16_table::<14, 16384, SKIP_START, true>(
                &mut self.batch,
                input,
                max_backward_distance,
            ),
            32768 => collect_with_stack_u16_table::<15, 32768, SKIP_START, true>(
                &mut self.batch,
                input,
                max_backward_distance,
            ),
            _ => return false,
        }
        true
    }

    #[allow(clippy::large_stack_arrays)]
    fn collect_with_64k_medium_skip(
        &mut self,
        input: &[u8],
        max_backward_distance: usize,
    ) -> &Batch {
        self.reset(input.len());

        if input.len() < INPUT_MARGIN_BYTES {
            self.push_literals(input, 0, input.len());
            return &self.batch;
        }

        let mut table = [0_u32; 1 << 16];
        collect_with_u32_table_m6::<16, { tune::Q1_MEDIUM_U32_SKIP_START }, true, true>(
            &mut self.batch,
            input,
            max_backward_distance,
            &mut table,
        );
        &self.batch
    }

    #[allow(clippy::large_stack_arrays)]
    fn collect_with_64k_fast_skip(&mut self, input: &[u8], max_backward_distance: usize) -> &Batch {
        self.reset(input.len());

        if input.len() < INPUT_MARGIN_BYTES {
            self.push_literals(input, 0, input.len());
            return &self.batch;
        }

        let mut table = [0_u32; 1 << 16];
        collect_with_u32_table_m6::<16, { tune::Q1_FAST_U32_SKIP_START }, false, true>(
            &mut self.batch,
            input,
            max_backward_distance,
            &mut table,
        );
        &self.batch
    }

    #[allow(clippy::large_stack_arrays)]
    fn collect_with_64k_sparse_stride(
        &mut self,
        input: &[u8],
        max_backward_distance: usize,
    ) -> &Batch {
        self.reset(input.len());

        if input.len() < INPUT_MARGIN_BYTES {
            self.push_literals(input, 0, input.len());
            return &self.batch;
        }

        let mut table = [NO_POSITION; 1 << 16];
        collect_with_u32_table_m6_sparse_stride::<16, 64>(
            &mut self.batch,
            input,
            max_backward_distance,
            &mut table,
        );
        &self.batch
    }

    #[allow(clippy::large_stack_arrays)]
    fn collect_with_32k_dense_skip(
        &mut self,
        input: &[u8],
        max_backward_distance: usize,
    ) -> &Batch {
        self.reset(input.len());

        if input.len() < INPUT_MARGIN_BYTES {
            self.push_literals(input, 0, input.len());
            return &self.batch;
        }

        let mut table = [0_u32; 1 << 15];
        collect_with_u32_table_m6::<15, { tune::Q1_DENSE_U32_SKIP_START }, true, false>(
            &mut self.batch,
            input,
            max_backward_distance,
            &mut table,
        );
        &self.batch
    }

    #[allow(clippy::large_stack_arrays)]
    fn collect_with_32k_u16_skip(&mut self, input: &[u8], max_backward_distance: usize) -> &Batch {
        self.reset(input.len());

        if input.len() < INPUT_MARGIN_BYTES {
            self.push_literals(input, 0, input.len());
            return &self.batch;
        }

        collect_with_stack_u16_table::<15, 32768, { tune::Q1_FASTER_U16_SKIP_START }, true>(
            &mut self.batch,
            input,
            max_backward_distance,
        );
        &self.batch
    }

    #[allow(clippy::large_stack_arrays)]
    fn collect_with_32k_faster_skip(
        &mut self,
        input: &[u8],
        max_backward_distance: usize,
    ) -> &Batch {
        self.reset(input.len());

        if input.len() < INPUT_MARGIN_BYTES {
            self.push_literals(input, 0, input.len());
            return &self.batch;
        }

        let mut table = [0_u32; 1 << 15];
        collect_with_u32_table_m6::<15, { tune::Q1_FASTER_U32_SKIP_START }, false, true>(
            &mut self.batch,
            input,
            max_backward_distance,
            &mut table,
        );
        &self.batch
    }

    fn reset(&mut self, input_len: usize) {
        self.batch.reset(input_len);
    }

    fn push_literals(&mut self, input: &[u8], insert_start: usize, insert_len: usize) {
        if insert_len == 0 {
            return;
        }
        self.batch.push_literals(input, insert_start, insert_len);
    }

    #[allow(clippy::too_many_arguments)]
    fn scan_to_match(
        &mut self,
        input: &[u8],
        pos: &mut usize,
        next_hash: &mut usize,
        len_limit: usize,
        max_distance: usize,
        table_bits: usize,
        min_match: usize,
        last_distance: usize,
    ) -> Option<usize> {
        let mut skip = 32_usize;
        let mut next_pos = *pos;

        loop {
            let key = *next_hash;
            let step = skip >> 5;
            skip += 1;
            *pos = next_pos;
            if step > len_limit - *pos {
                return None;
            }
            next_pos = *pos + step;
            *next_hash = hash(input, next_pos, table_bits, min_match);

            if last_distance != NO_LAST_DISTANCE && *pos >= last_distance {
                let candidate = *pos - last_distance;
                if is_match(input, candidate, *pos, min_match) {
                    self.table[key] = *pos as u32;
                    return Some(candidate);
                }
            }

            let candidate = self.table[key] as usize;
            self.table[key] = *pos as u32;
            if candidate != NO_POSITION as usize
                && candidate < *pos
                && *pos - candidate <= max_distance
                && is_match(input, candidate, *pos, min_match)
            {
                return Some(candidate);
            }
        }
    }

    fn store_tail_hashes(
        &mut self,
        input: &[u8],
        copy_end: usize,
        table_bits: usize,
        min_match: usize,
    ) {
        if min_match == 4 && copy_end >= 3 && copy_end + 5 <= input.len() {
            let word = read_u64_le(input, copy_end - 3);
            for offset in 0..3 {
                let key = hash_word_at_offset(word, offset, table_bits, min_match);
                self.table[key] = (copy_end - 3 + offset) as u32;
            }
            return;
        }
        if min_match == 6 && copy_end >= 5 && copy_end + 6 <= input.len() {
            let word = read_u64_le(input, copy_end - 5);
            for offset in 0..3 {
                let key = hash_word_at_offset(word, offset, table_bits, min_match);
                self.table[key] = (copy_end - 5 + offset) as u32;
            }
            let word = read_u64_le(input, copy_end - 2);
            for offset in 0..2 {
                let key = hash_word_at_offset(word, offset, table_bits, min_match);
                self.table[key] = (copy_end - 2 + offset) as u32;
            }
            return;
        }

        let start = copy_end.saturating_sub(min_match - 1);
        for pos in start..copy_end {
            if pos + 8 > input.len() {
                break;
            }
            let key = hash(input, pos, table_bits, min_match);
            self.table[key] = pos as u32;
        }
    }
}

fn collect_with_u16_table(
    batch: &mut Batch,
    input: &[u8],
    max_backward_distance: usize,
    table: &mut [u16],
    table_bits: usize,
) -> Result<(), CompressError> {
    match table_bits {
        14 => {
            let table: &mut [u16; 16384] = table
                .try_into()
                .map_err(|_| BurliError::Format("invalid Brotli q1 table size"))?;
            collect_with_u16_table_m4::<14, 16384, { tune::Q1_DEFAULT_U16_SKIP_START }, true>(
                batch,
                input,
                max_backward_distance,
                table,
            );
            Ok(())
        }
        15 => {
            let table: &mut [u16; 32768] = table
                .try_into()
                .map_err(|_| BurliError::Format("invalid Brotli q1 table size"))?;
            collect_with_u16_table_m4::<15, 32768, { tune::Q1_DEFAULT_U16_SKIP_START }, true>(
                batch,
                input,
                max_backward_distance,
                table,
            );
            Ok(())
        }
        _ => Err(BurliError::Format("invalid Brotli q1 table size")),
    }
}

#[allow(clippy::large_stack_arrays)]
#[inline(never)]
fn collect_with_stack_u16_table<
    const TABLE_BITS: usize,
    const TABLE_LEN: usize,
    const SKIP_START: usize,
    const USE_LAST_DISTANCE: bool,
>(
    batch: &mut Batch,
    input: &[u8],
    max_backward_distance: usize,
) {
    let mut table = [NO_POSITION_16; TABLE_LEN];
    collect_with_u16_table_m4::<TABLE_BITS, TABLE_LEN, SKIP_START, USE_LAST_DISTANCE>(
        batch,
        input,
        max_backward_distance,
        &mut table,
    );
}

fn collect_with_u32_table_m6<
    const TABLE_BITS: usize,
    const SKIP_START: usize,
    const USE_LAST_DISTANCE: bool,
    const STORE_TAIL_HASHES: bool,
>(
    batch: &mut Batch,
    input: &[u8],
    max_backward_distance: usize,
    table: &mut [u32],
) {
    debug_assert_eq!(table.len(), 1_usize << TABLE_BITS);
    let max_distance = max_backward_distance.min(MAX_DISTANCE);
    let len_limit = input
        .len()
        .saturating_sub(6)
        .min(input.len().saturating_sub(INPUT_MARGIN_BYTES));
    if len_limit <= 1 {
        push_literals_to_batch(batch, input, 0, input.len());
        return;
    }

    let mut pos = 1;
    let mut insert_start = 0;
    let mut next_hash = hash6_at_const::<TABLE_BITS>(input, pos);
    let mut last_distance = NO_LAST_DISTANCE;

    while let Some(mut candidate) =
        scan_to_match_in_u32_table_m6::<TABLE_BITS, SKIP_START, USE_LAST_DISTANCE>(
            table,
            input,
            &mut pos,
            &mut next_hash,
            len_limit,
            max_distance,
            last_distance,
        )
    {
        loop {
            let distance = pos - candidate;
            let max_copy_len = (MAX_META_BLOCK_SIZE - (pos - insert_start)).min(input.len() - pos);
            if max_copy_len < 6 {
                break;
            }
            let copy_len = 6 + match_len(input, candidate + 6, pos + 6, max_copy_len - 6);

            batch.push_copy(
                input,
                insert_start,
                pos - insert_start,
                copy_len,
                distance,
                last_distance,
            );
            pos += copy_len;
            insert_start = pos;
            last_distance = distance;

            if pos > len_limit {
                break;
            }

            if STORE_TAIL_HASHES {
                store_tail_hashes_in_u32_table_m6::<TABLE_BITS>(table, input, pos);
            }

            let key = hash6_at_const::<TABLE_BITS>(input, pos);
            candidate = table[key] as usize;
            table[key] = pos as u32;
            if candidate >= pos
                || pos - candidate > max_distance
                || !is_match6(input, candidate, pos)
            {
                break;
            }
        }

        pos += 1;
        if pos > len_limit {
            break;
        }
        next_hash = hash6_at_const::<TABLE_BITS>(input, pos);
    }

    if insert_start < input.len() {
        push_literals_to_batch(batch, input, insert_start, input.len() - insert_start);
    }
}

fn collect_with_u32_table_m6_sparse_stride<const TABLE_BITS: usize, const SPARSE_STRIDE: usize>(
    batch: &mut Batch,
    input: &[u8],
    max_backward_distance: usize,
    table: &mut [u32],
) {
    debug_assert_eq!(table.len(), 1_usize << TABLE_BITS);
    debug_assert!(SPARSE_STRIDE.is_power_of_two());
    let max_distance = max_backward_distance.min(MAX_DISTANCE);
    let len_limit = input
        .len()
        .saturating_sub(6)
        .min(input.len().saturating_sub(INPUT_MARGIN_BYTES));
    if len_limit <= 1 {
        push_literals_to_batch(batch, input, 0, input.len());
        return;
    }

    let mut pos = 0;
    let mut insert_start = 0;
    let mut last_distance = NO_LAST_DISTANCE;

    while pos <= len_limit {
        let key = hash6_at_const::<TABLE_BITS>(input, pos);
        let candidate = if last_distance != NO_LAST_DISTANCE
            && pos >= last_distance
            && is_match6(input, pos - last_distance, pos)
        {
            table[key] = pos as u32;
            Some(pos - last_distance)
        } else {
            let candidate = table[key] as usize;
            table[key] = pos as u32;
            (candidate < pos && pos - candidate <= max_distance && is_match6(input, candidate, pos))
                .then_some(candidate)
        };

        if let Some(candidate) = candidate {
            let distance = pos - candidate;
            let max_copy_len = (MAX_META_BLOCK_SIZE - (pos - insert_start)).min(input.len() - pos);
            if max_copy_len >= 6 {
                let copy_len = 6 + match_len(input, candidate + 6, pos + 6, max_copy_len - 6);
                batch.push_copy(
                    input,
                    insert_start,
                    pos - insert_start,
                    copy_len,
                    distance,
                    last_distance,
                );
                pos += copy_len;
                insert_start = pos;
                last_distance = distance;
                pos = pos.saturating_add(SPARSE_STRIDE - 1) & !(SPARSE_STRIDE - 1);
                continue;
            }
        }

        pos += SPARSE_STRIDE;
    }

    if insert_start < input.len() {
        push_literals_to_batch(batch, input, insert_start, input.len() - insert_start);
    }
}

fn collect_with_u16_table_m4<
    const TABLE_BITS: usize,
    const TABLE_LEN: usize,
    const SKIP_START: usize,
    const USE_LAST_DISTANCE: bool,
>(
    batch: &mut Batch,
    input: &[u8],
    _max_backward_distance: usize,
    table: &mut [u16; TABLE_LEN],
) {
    debug_assert_eq!(TABLE_LEN, 1_usize << TABLE_BITS);
    let len_limit = input
        .len()
        .saturating_sub(4)
        .min(input.len().saturating_sub(INPUT_MARGIN_BYTES));
    if len_limit <= 1 {
        push_literals_to_batch(batch, input, 0, input.len());
        return;
    }

    let mut pos = 1;
    let mut insert_start = 0;
    let mut next_word = read_u32_le(input, pos);
    let mut next_hash = hash4_const::<TABLE_BITS>(next_word);
    let mut last_distance = NO_LAST_DISTANCE;

    while let Some(mut candidate) =
        scan_to_match_in_u16_table_m4::<TABLE_BITS, TABLE_LEN, SKIP_START, USE_LAST_DISTANCE>(
            table,
            input,
            &mut pos,
            &mut next_word,
            &mut next_hash,
            len_limit,
            last_distance,
        )
    {
        loop {
            let distance = pos - candidate;
            let max_copy_len = (MAX_META_BLOCK_SIZE - (pos - insert_start)).min(input.len() - pos);
            if max_copy_len < 4 {
                break;
            }
            let copy_len = 4 + match_len(input, candidate + 4, pos + 4, max_copy_len - 4);

            batch.push_copy(
                input,
                insert_start,
                pos - insert_start,
                copy_len,
                distance,
                last_distance,
            );
            pos += copy_len;
            insert_start = pos;
            last_distance = distance;

            if pos > len_limit {
                break;
            }

            let current_key =
                store_tail_hashes_in_u16_table_m4::<TABLE_BITS, TABLE_LEN>(table, input, pos);
            let current_word = read_u32_le(input, pos);
            let key = current_key.unwrap_or_else(|| hash4_const::<TABLE_BITS>(current_word));
            let entry = table[key];
            table[key] = position_to_u16_entry(pos);
            let next_candidate = usize::from(entry);
            debug_assert!(next_candidate < pos);
            if read_u32_le(input, next_candidate) != current_word {
                break;
            }
            candidate = next_candidate;
        }

        pos += 1;
        if pos > len_limit {
            break;
        }
        next_word = read_u32_le(input, pos);
        next_hash = hash4_const::<TABLE_BITS>(next_word);
    }

    if insert_start < input.len() {
        push_literals_to_batch(batch, input, insert_start, input.len() - insert_start);
    }
}

#[inline(always)]
#[allow(clippy::too_many_arguments)]
fn scan_to_match_in_u32_table_m6<
    const TABLE_BITS: usize,
    const SKIP_START: usize,
    const USE_LAST_DISTANCE: bool,
>(
    table: &mut [u32],
    input: &[u8],
    pos: &mut usize,
    next_hash: &mut usize,
    len_limit: usize,
    max_distance: usize,
    last_distance: usize,
) -> Option<usize> {
    let mut skip = SKIP_START;
    let mut next_pos = *pos;

    if !USE_LAST_DISTANCE || last_distance == NO_LAST_DISTANCE {
        loop {
            let key = *next_hash;
            let step = skip >> 5;
            skip += 1;
            *pos = next_pos;
            if step > len_limit - *pos {
                return None;
            }
            next_pos = *pos + step;
            *next_hash = hash6_at_const::<TABLE_BITS>(input, next_pos);

            let candidate = table[key] as usize;
            table[key] = *pos as u32;
            if candidate < *pos
                && *pos - candidate <= max_distance
                && is_match6(input, candidate, *pos)
            {
                return Some(candidate);
            }
        }
    }

    loop {
        let key = *next_hash;
        let step = skip >> 5;
        skip += 1;
        *pos = next_pos;
        if step > len_limit - *pos {
            return None;
        }
        next_pos = *pos + step;
        *next_hash = hash6_at_const::<TABLE_BITS>(input, next_pos);

        if *pos >= last_distance {
            let candidate = *pos - last_distance;
            if is_match6(input, candidate, *pos) {
                table[key] = *pos as u32;
                return Some(candidate);
            }
        }

        let candidate = table[key] as usize;
        table[key] = *pos as u32;
        if candidate < *pos && *pos - candidate <= max_distance && is_match6(input, candidate, *pos)
        {
            return Some(candidate);
        }
    }
}

#[inline(always)]
fn store_tail_hashes_in_u32_table_m6<const TABLE_BITS: usize>(
    table: &mut [u32],
    input: &[u8],
    copy_end: usize,
) {
    if copy_end >= 5 && copy_end + 6 <= input.len() {
        let word = read_u64_le(input, copy_end - 5);
        for offset in 0..3 {
            let key = hash6_word_at_offset_const::<TABLE_BITS>(word, offset);
            table[key] = (copy_end - 5 + offset) as u32;
        }
        let word = read_u64_le(input, copy_end - 2);
        for offset in 0..2 {
            let key = hash6_word_at_offset_const::<TABLE_BITS>(word, offset);
            table[key] = (copy_end - 2 + offset) as u32;
        }
        return;
    }

    let start = copy_end.saturating_sub(5);
    for pos in start..copy_end {
        if pos + 8 > input.len() {
            break;
        }
        let key = hash6_at_const::<TABLE_BITS>(input, pos);
        table[key] = pos as u32;
    }
}

#[inline(always)]
fn push_literals_to_batch(batch: &mut Batch, input: &[u8], insert_start: usize, insert_len: usize) {
    if insert_len == 0 {
        return;
    }
    batch.push_literals(input, insert_start, insert_len);
}

#[inline(always)]
#[allow(clippy::too_many_arguments)]
fn scan_to_match_in_u16_table_m4<
    const TABLE_BITS: usize,
    const TABLE_LEN: usize,
    const SKIP_START: usize,
    const USE_LAST_DISTANCE: bool,
>(
    table: &mut [u16; TABLE_LEN],
    input: &[u8],
    pos: &mut usize,
    next_word: &mut u32,
    next_hash: &mut usize,
    len_limit: usize,
    last_distance: usize,
) -> Option<usize> {
    let mut skip = SKIP_START;
    let mut next_pos = *pos;

    if !USE_LAST_DISTANCE || last_distance == NO_LAST_DISTANCE {
        loop {
            let key = *next_hash;
            let step = skip >> 5;
            skip += 1;
            *pos = next_pos;
            if step > len_limit - *pos {
                return None;
            }
            next_pos = *pos + step;
            let current_word = *next_word;
            *next_word = read_u32_le(input, next_pos);
            *next_hash = hash4_const::<TABLE_BITS>(*next_word);

            let entry = table[key];
            table[key] = position_to_u16_entry(*pos);
            let candidate = usize::from(entry);
            debug_assert!(candidate < *pos);
            if read_u32_le(input, candidate) == current_word {
                return Some(candidate);
            }
        }
    }

    loop {
        let key = *next_hash;
        let step = skip >> 5;
        skip += 1;
        *pos = next_pos;
        if step > len_limit - *pos {
            return None;
        }
        next_pos = *pos + step;
        let current_word = *next_word;
        *next_word = read_u32_le(input, next_pos);
        *next_hash = hash4_const::<TABLE_BITS>(*next_word);

        if *pos >= last_distance {
            let candidate = *pos - last_distance;
            if read_u32_le(input, candidate) == current_word {
                table[key] = position_to_u16_entry(*pos);
                return Some(candidate);
            }
        }

        let entry = table[key];
        table[key] = position_to_u16_entry(*pos);
        let candidate = usize::from(entry);
        debug_assert!(candidate < *pos);
        if read_u32_le(input, candidate) == current_word {
            return Some(candidate);
        }
    }
}

#[inline(always)]
fn store_tail_hashes_in_u16_table_m4<const TABLE_BITS: usize, const TABLE_LEN: usize>(
    table: &mut [u16; TABLE_LEN],
    input: &[u8],
    copy_end: usize,
) -> Option<usize> {
    if copy_end >= 3 && copy_end + 5 <= input.len() {
        let word = read_u64_le(input, copy_end - 3);
        for offset in 0..3 {
            let key = hash4_const::<TABLE_BITS>((word >> (8 * offset)) as u32);
            table[key] = position_to_u16_entry(copy_end - 3 + offset);
        }
        return Some(hash4_const::<TABLE_BITS>((word >> 24) as u32));
    }

    let start = copy_end.saturating_sub(3);
    for pos in start..copy_end {
        if pos + 4 > input.len() {
            break;
        }
        let key = hash4_at_const::<TABLE_BITS>(input, pos);
        table[key] = position_to_u16_entry(pos);
    }
    None
}

#[inline(always)]
fn position_to_u16_entry(pos: usize) -> u16 {
    debug_assert!(u16::try_from(pos).is_ok());
    pos as u16
}

fn table_size(input_len: usize) -> usize {
    let mut size = MIN_TABLE_SIZE;
    while size < MAX_TABLE_SIZE && size < input_len {
        size <<= 1;
    }
    size
}

#[inline(always)]
fn hash(input: &[u8], pos: usize, table_bits: usize, min_match: usize) -> usize {
    if min_match == 4 {
        return hash4(read_u32_le(input, pos), table_bits);
    }
    let word = read_u64_le(input, pos) << ((8 - min_match) * 8);
    ((word.wrapping_mul(HASH_MUL)) >> (64 - table_bits)) as usize
}

#[inline(always)]
fn hash_word_at_offset(word: u64, offset: usize, table_bits: usize, min_match: usize) -> usize {
    if min_match == 4 {
        return hash4((word >> (8 * offset)) as u32, table_bits);
    }
    let shifted = (word >> (8 * offset)) << ((8 - min_match) * 8);
    ((shifted.wrapping_mul(HASH_MUL)) >> (64 - table_bits)) as usize
}

#[inline(always)]
fn hash4(word: u32, table_bits: usize) -> usize {
    (word.wrapping_mul(HASH_MUL_32) >> (32 - table_bits)) as usize
}

#[inline(always)]
fn hash4_const<const TABLE_BITS: usize>(word: u32) -> usize {
    (word.wrapping_mul(HASH_MUL_32) >> (32 - TABLE_BITS)) as usize
}

#[inline(always)]
fn hash4_at_const<const TABLE_BITS: usize>(input: &[u8], pos: usize) -> usize {
    hash4_const::<TABLE_BITS>(read_u32_le(input, pos))
}

#[inline(always)]
fn hash6_const<const TABLE_BITS: usize>(word: u64) -> usize {
    ((word << 16).wrapping_mul(HASH_MUL) >> (64 - TABLE_BITS)) as usize
}

#[inline(always)]
fn hash6_at_const<const TABLE_BITS: usize>(input: &[u8], pos: usize) -> usize {
    hash6_const::<TABLE_BITS>(read_u64_le(input, pos))
}

#[inline(always)]
fn hash6_word_at_offset_const<const TABLE_BITS: usize>(word: u64, offset: usize) -> usize {
    hash6_const::<TABLE_BITS>(word >> (8 * offset))
}

#[inline(always)]
fn read_u32_le(input: &[u8], pos: usize) -> u32 {
    debug_assert!(pos.checked_add(4).is_some_and(|end| end <= input.len()));
    read_u32_le_trusted(input, pos)
}

#[inline(always)]
fn read_u32_le_trusted(input: &[u8], pos: usize) -> u32 {
    super::load::read_u32_le_trusted(input, pos)
}

#[inline(always)]
fn is_match6(input: &[u8], candidate: usize, pos: usize) -> bool {
    const LOW_48_BITS: u64 = 0x0000_ffff_ffff_ffff;
    let diff = read_u64_le(input, candidate) ^ read_u64_le(input, pos);
    diff & LOW_48_BITS == 0
}

#[inline(always)]
fn is_match(input: &[u8], candidate: usize, pos: usize, min_match: usize) -> bool {
    if min_match == 4 {
        read_u32_le(input, candidate) == read_u32_le(input, pos)
    } else {
        const LOW_48_BITS: u64 = 0x0000_ffff_ffff_ffff;
        let diff = read_u64_le(input, candidate) ^ read_u64_le(input, pos);
        diff & LOW_48_BITS == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trusted_u32_load_matches_safe_little_endian_chunks() {
        let input: Vec<u8> = (0..=255).cycle().take(512).collect();
        for pos in 0..=input.len() - 4 {
            let expected = u32::from_le_bytes(input[pos..pos + 4].try_into().unwrap());

            assert_eq!(read_u32_le(&input, pos), expected);
        }
    }
}
