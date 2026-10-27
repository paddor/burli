//! One-pass q0 encoder.
//!
//! Like Google's fast fragment encoder, it writes each match as soon as it
//! finds it, so there is no command buffer and no second pass. An input up
//! to [`MAX_INPUT`] takes one meta-block, with a literal code from the
//! histogram of the whole input and one of the fixed codes in
//! [`super::fixed_codes`] for commands and distances, by input size. Longer
//! inputs take meta-blocks of [`LONG_BLOCK`] bytes, which copy from earlier
//! ones. Their literal codes come from a sample of each block, and their
//! command and distance codes from the counts of the block before. Blocks
//! that look incompressible take only literals, or are stored.

use alloc::{boxed::Box, vec};

use burli_core::{
    CompressError,
    bits::{BitSink, BitWriter, MAX_BITS_PER_OP},
};

use super::{
    COMMAND_ALPHABET_SIZE, DistanceRing, FAST_CODE_BITS, LITERAL_ALPHABET_SIZE, MAX_CODE_BITS,
    context_literals::{COST_SHIFT, n_log2_n},
    fixed_codes::{
        Code, CodeTables, Flat, LONG_COMMAND_LENGTHS, LONG_DISTANCE_LENGTHS, Long, Short,
        explicit_symbol, implicit_symbol,
    },
    hash_word_q0,
    prefix_code::{self, LiteralCodes},
    read_u64_le, write_block_and_context_header, write_meta_block_len,
    write_simple_prefix_code_single,
};

/// Longest input of [`write`]. Hash table entries hold `u16` positions.
pub(super) const MAX_INPUT: usize = 1 << 16;
const _: () = assert!(super::tune::Q0_ONE_PASS_MAX_INPUT <= MAX_INPUT);

/// Meta-blocks of [`write_long`] take this many bytes.
const LONG_BLOCK: usize = 1 << 18;
/// Hash table entries of [`write_long`].
const LONG_TABLE_SIZE: usize = 1 << 15;
/// Bits of a position that hash table entries of [`write_long`] keep.
const LONG_REACH_BITS: u32 = 18;
/// The farthest copies of [`write_long`] reach, which the window must allow.
pub(super) const LONG_MAX_DISTANCE: usize = 1 << LONG_REACH_BITS;
/// The distance symbols that copies of [`write_long`] can take: 0 for the
/// last distance, and from 16 the symbols up to [`LONG_MAX_DISTANCE`].
const LONG_DISTANCE_SYMBOLS: usize = 16 + 2 * (LONG_REACH_BITS as usize - 1);

/// [`write_long`] counts every this many bytes for literal codes.
const LITERAL_SAMPLE_STRIDE: usize = 29;
/// [`write_long`] counts the first [`LITERAL_ONLY_SAMPLE`] bytes of every
/// this many of a low-compressibility block for its literal-only code. Runs
/// of bytes take every field of short records alike.
const LITERAL_ONLY_PERIOD: usize = 1024;
const LITERAL_ONLY_SAMPLE: usize = 64;
/// Literal-only codes take at most this many bits, so a group of
/// `LITERAL_ONLY_GROUP` fits one write, after up to 7 waiting bits.
const LITERAL_ONLY_CODE_BITS: u8 = 11;
const LITERAL_ONLY_GROUP: usize = 5;
const _: () = assert!(LITERAL_ONLY_GROUP * LITERAL_ONLY_CODE_BITS as usize + 7 <= 64);
/// [`write_long`] stores low-compressibility blocks whose literal-only code
/// would save fewer than 1 in this many bits.
const LITERAL_ONLY_MIN_SAVING: u64 = 32;
/// Literal-only meta-blocks go out in runs of this many literals, with one
/// room check per run. A run and the up to 7 bits that wait before it take
/// at most 56 bytes, so its 8-byte stores start at most 56 bytes in.
const LITERAL_ONLY_RUN: usize = 40;
const LITERAL_ONLY_WINDOW: usize = 72;
const _: () = assert!(
    (LITERAL_ONLY_RUN * LITERAL_ONLY_CODE_BITS as usize + 7).div_ceil(8) + 8 <= 64
        && 64 + 8 <= LITERAL_ONLY_WINDOW
);

const MIN_MATCH: usize = 5;
/// Matching stops this many bytes before the end, so every hash load reads
/// 8 bytes in bounds.
const INPUT_MARGIN: usize = 8;
/// Literals go out in runs of this many, with one room check per run.
const LITERAL_RUN: usize = 32;
/// A command code with its insert extra bits, then its copy extra bits.
const COMMAND_MAX_BITS: usize = MAX_CODE_BITS as usize + 24 + 24;
/// A distance code with its extra bits.
const DISTANCE_MAX_BITS: usize = MAX_CODE_BITS as usize + 24;

/// Writes `input` as one compressed meta-block and advances `ring` past its
/// copies. Returns `false` with nothing written when its commands would take
/// more bytes than `input`, so the caller can store it instead.
pub(super) fn write(
    writer: &mut BitWriter,
    input: &[u8],
    max_distance: usize,
    ring: &mut DistanceRing,
) -> Result<bool, CompressError> {
    // Copies stay inside `input`, so they never reach past the window.
    debug_assert!(input.len() <= MAX_INPUT && input.len() <= max_distance);
    // Hash tables with an odd number of index bits, as in Google's encoder.
    match input.len() {
        0..=512 => write_with::<{ 1 << 9 }, Flat>(writer, input, ring),
        513..=2048 => write_with::<{ 1 << 11 }, Flat>(writer, input, ring),
        2049..=4096 => write_with::<{ 1 << 13 }, Flat>(writer, input, ring),
        4097..=8192 => write_with::<{ 1 << 13 }, Short>(writer, input, ring),
        8193..=16384 => write_with::<{ 1 << 15 }, Short>(writer, input, ring),
        _ => write_with::<{ 1 << 15 }, Long>(writer, input, ring),
    }
}

/// [`write`] with a hash table of `TABLE_SIZE` entries and the fixed code
/// `C`, whose tables then sit at a constant address. Each size gets its
/// own function, so register allocation in one does not shape the others.
#[inline(never)]
fn write_with<const TABLE_SIZE: usize, C: Code>(
    writer: &mut BitWriter,
    input: &[u8],
    ring: &mut DistanceRing,
) -> Result<bool, CompressError> {
    let mark = writer.mark();
    write_meta_block_len(writer, input.len())?;
    write_block_and_context_header(writer)?;
    let literals = write_literal_code(writer, input)?;
    C::CODE.write_header(writer);
    let mut table = [0_u16; TABLE_SIZE];
    let fits = write_commands(
        writer,
        input,
        0,
        &mut table,
        &literals,
        &C::CODE.tables,
        ring,
        &mut (),
    );
    if !fits {
        writer.rewind(mark);
    }
    Ok(fits)
}

/// Writes long `input` as meta-blocks of [`LONG_BLOCK`] bytes, and advances
/// `ring` past their copies. Copies reach into earlier meta-blocks, up to
/// [`LONG_MAX_DISTANCE`] back, which the window must allow. Each block takes
/// command and distance codes for the counts of the block before. Blocks
/// that look incompressible take a literal-only meta-block when a code
/// from a sample of them saves enough, and are stored otherwise, as are
/// blocks that do not shrink.
pub(super) fn write_long(
    writer: &mut BitWriter,
    input: &[u8],
    ring: &mut DistanceRing,
) -> Result<(), CompressError> {
    let mut table: Box<[u32; LONG_TABLE_SIZE]> = vec![0; LONG_TABLE_SIZE]
        .try_into()
        .expect("table of LONG_TABLE_SIZE entries");
    // Counts of the block before, unless `fresh`.
    let mut previous = Counts::default();
    let mut fresh = true;
    let mut current = Counts::default();
    // Where the blocks to store from here on start, so they go out as one
    // meta-block.
    let mut stored_start = None;
    let mut start = 0;
    while start < input.len() {
        let end = (start + LONG_BLOCK).min(input.len());
        let block = &input[start..end];
        let low = super::sparse::is_low_compressibility_block(block);
        let literal_counts = if low {
            literal_only_counts(block)
        } else {
            None
        };
        let stored = (low && literal_counts.is_none()) || {
            if let Some(stored_start) = stored_start.take() {
                write_stored(writer, &input[stored_start..start])?;
            }
            let mark = writer.mark();
            let fits = if let Some(counts) = &literal_counts {
                write_literal_block(writer, block, counts)?
            } else {
                write_meta_block_len(writer, block.len())?;
                write_block_and_context_header(writer)?;
                let literals = write_sampled_literal_code(writer, block)?;
                let code = if fresh { &INITIAL_COUNTS } else { &previous }.write_codes(writer)?;
                current = Counts::default();
                write_long_block(
                    writer,
                    &input[..end],
                    start,
                    &mut table,
                    &literals,
                    &code,
                    ring,
                    &mut current,
                )
            };
            if !fits {
                writer.rewind(mark);
            }
            !fits
        };
        if stored {
            stored_start.get_or_insert(start);
            fresh = true;
        } else if low {
            fresh = true;
        } else {
            core::mem::swap(&mut previous, &mut current);
            fresh = false;
        }
        start = end;
    }
    if let Some(stored_start) = stored_start {
        write_stored(writer, &input[stored_start..])?;
    }
    Ok(())
}

/// Counts of the bytes of `block` that [`LITERAL_ONLY_PERIOD`] picks, with
/// each byte value counted once more, when a code for them saves at least 1
/// in [`LITERAL_ONLY_MIN_SAVING`] bits by their entropy.
fn literal_only_counts(block: &[u8]) -> Option<[u32; LITERAL_ALPHABET_SIZE]> {
    // Four counters per byte, so repeats of one byte do not chain every
    // increment to the one before it.
    let mut tables = [[0_u32; LITERAL_ALPHABET_SIZE]; 4];
    for period in block.chunks(LITERAL_ONLY_PERIOD) {
        let (quads, _) = period[..period.len().min(LITERAL_ONLY_SAMPLE)].as_chunks::<4>();
        for quad in quads {
            for (table, &byte) in tables.iter_mut().zip(quad) {
                table[usize::from(byte)] += 1;
            }
        }
    }
    let counts: [u32; LITERAL_ALPHABET_SIZE] =
        core::array::from_fn(|byte| 1 + tables.iter().map(|table| table[byte]).sum::<u32>());
    let total = counts.iter().sum::<u32>();
    let entropy =
        n_log2_n(total).saturating_sub(counts.iter().map(|&count| n_log2_n(count)).sum::<u64>());
    let stored = (u64::from(total) * 8) << COST_SHIFT;
    (entropy * LITERAL_ONLY_MIN_SAVING < stored * (LITERAL_ONLY_MIN_SAVING - 1)).then_some(counts)
}

/// Writes `block` as a literal-only meta-block with a literal code for
/// `counts`. Returns `false` when that takes more bytes than storing the
/// block, so the caller can store it instead.
fn write_literal_block(
    writer: &mut BitWriter,
    block: &[u8],
    counts: &[u32; LITERAL_ALPHABET_SIZE],
) -> Result<bool, CompressError> {
    let header_start = writer.written_bits();
    write_meta_block_len(writer, block.len())?;
    write_block_and_context_header(writer)?;
    let (codes, _) = prefix_code::write_with_max_bits::<_, LITERAL_ONLY_CODE_BITS>(writer, counts)?;
    // The meta-block ends after the literals, so the copy length is never
    // used and no distance follows.
    let (insert_code, insert_bits, insert_extra) = insert_code(block.len());
    let symbol = if insert_code < 8 {
        implicit_symbol(insert_code, 0)
    } else {
        explicit_symbol(insert_code, 0)
    };
    write_simple_prefix_code_single(writer, COMMAND_ALPHABET_SIZE, symbol as u16)?;
    write_simple_prefix_code_single(writer, 64, 0)?;
    writer.write_bits(insert_bits, insert_extra)?;
    // A stored block takes at most 4 header bytes.
    let header_bits = writer.written_bits() - header_start;
    let Some(max_bits) = ((block.len() + 4) * 8).checked_sub(header_bits) else {
        return Ok(false);
    };
    // Code bits and lengths by literal, apart, so a literal takes two loads
    // and no unpacking.
    let code_bits: [u64; LITERAL_ALPHABET_SIZE] = codes.map(|code| u64::from(code >> 8));
    let code_lens: [u8; LITERAL_ALPHABET_SIZE] = codes.map(|code| code as u8);
    // The bits from the partial byte on wait in `bits`, and each store
    // writes all 8 bytes of them, so it does not wait on the store before
    // it. The stores of a run stay in a window of `LITERAL_ONLY_WINDOW`
    // bytes, at offsets that need no bounds check.
    let fits = writer.write_bits_raw(max_bits, |out, start| {
        let mut literals = Literals {
            pos: 0,
            bits: u64::from(out[0]),
            count: start as u32,
        };
        // Full runs have a fixed length, so their groups unroll.
        let (runs, tail) = block.as_chunks::<LITERAL_ONLY_RUN>();
        let fits = runs
            .iter()
            .all(|run| literals.write_run(out, run, &code_bits, &code_lens))
            && (tail.is_empty() || literals.write_run(out, tail, &code_bits, &code_lens));
        (literals.pos * 8 + literals.count as usize, fits)
    });
    Ok(fits)
}

/// Output state of [`write_literal_block`]. The bits from the partial byte
/// on wait in `bits`, and each store writes all 8 bytes of them, so it does
/// not wait on the store before it.
struct Literals {
    /// Bytes before the partial one.
    pos: usize,
    bits: u64,
    /// Bits in `bits`, fewer than 8 between runs.
    count: u32,
}

impl Literals {
    /// Writes `run`, of at most [`LITERAL_ONLY_RUN`] literals, with their
    /// code bits and lengths. Its stores stay in a window of
    /// [`LITERAL_ONLY_WINDOW`] bytes, at offsets that need no bounds check.
    /// Returns `false` when `out` has no room for the window.
    #[inline(always)]
    fn write_run(
        &mut self,
        out: &mut [u8],
        run: &[u8],
        code_bits: &[u64; LITERAL_ALPHABET_SIZE],
        code_lens: &[u8; LITERAL_ALPHABET_SIZE],
    ) -> bool {
        let Some(window) = out.get_mut(self.pos..self.pos + LITERAL_ONLY_WINDOW) else {
            return false;
        };
        let window: &mut [u8; LITERAL_ONLY_WINDOW] = window.try_into().unwrap();
        let mut offset = 0;
        // Fewer than 8 bits wait, so a group of literals more fits in
        // `bits`. Each group comes together on its own, so its shifts do not
        // wait on the group before.
        let mut write = |group: &[u8]| {
            let mut value = 0_u64;
            let mut width = 0_u32;
            for &byte in group {
                value |= code_bits[usize::from(byte)] << width;
                width += u32::from(code_lens[usize::from(byte)]);
            }
            self.bits |= value << self.count;
            self.count += width;
            window[offset & 63..][..8].copy_from_slice(&self.bits.to_le_bytes());
            offset += (self.count / 8) as usize;
            self.bits >>= self.count & !7;
            self.count %= 8;
        };
        let (groups, rest) = run.as_chunks::<LITERAL_ONLY_GROUP>();
        for group in groups {
            write(group);
        }
        if !rest.is_empty() {
            write(rest);
        }
        self.pos += offset;
        true
    }
}

/// Writes `input` as stored meta-blocks, as few as their size limit allows.
fn write_stored(writer: &mut BitWriter, input: &[u8]) -> Result<(), CompressError> {
    for chunk in input.chunks(super::MAX_META_BLOCK_SIZE) {
        crate::metablock::write_uncompressed_meta_block(writer, chunk)?;
    }
    Ok(())
}

#[inline(never)]
#[allow(clippy::too_many_arguments)]
fn write_long_block(
    writer: &mut BitWriter,
    input: &[u8],
    start: usize,
    table: &mut [u32; LONG_TABLE_SIZE],
    literals: &LiteralCodes,
    code: &CodeTables,
    ring: &mut DistanceRing,
    counts: &mut Counts,
) -> bool {
    write_commands(writer, input, start, table, literals, code, ring, counts)
}

/// Writes `input[start..]` as the commands of one meta-block with `code`,
/// and advances `ring` past its copies into all of `input`. `table` holds
/// positions by hash, see [`Entry`], and `tally` counts the symbols.
/// Returns `false` when the commands would take more bytes than the
/// meta-block.
#[inline(always)]
#[allow(clippy::too_many_arguments)]
fn write_commands<E: Entry, T: Tally, const TABLE_SIZE: usize>(
    writer: &mut BitWriter,
    input: &[u8],
    start: usize,
    table: &mut [E; TABLE_SIZE],
    literals: &LiteralCodes,
    code: &CodeTables,
    ring: &mut DistanceRing,
    tally: &mut T,
) -> bool {
    let (fits, new_ring) = writer.write_bits_up_to((input.len() - start) * 8, |sink| {
        let mut out = Commands {
            sink,
            literals,
            code,
            ring: *ring,
            tally,
        };
        let fits = out.write_matches(input, start, table);
        (out.sink, (fits, out.ring))
    });
    if fits {
        *ring = new_ring;
    }
    fits
}

/// A hash table entry for a position.
trait Entry: Copy {
    fn new(pos: usize) -> Self;
    /// The position this entry stands for at `ip`.
    fn position(self, ip: usize) -> usize;
}

/// Whole positions of inputs up to [`MAX_INPUT`].
impl Entry for u16 {
    #[inline(always)]
    fn new(pos: usize) -> Self {
        pos as Self
    }

    #[inline(always)]
    fn position(self, _ip: usize) -> usize {
        usize::from(self)
    }
}

/// Positions of which only the low [`LONG_REACH_BITS`] count: they stand
/// for the latest position with those bits before `ip`. That is 1 to
/// [`LONG_MAX_DISTANCE`] back, and at most `ip` back, as entries start at
/// zero, so copies need no distance check.
impl Entry for u32 {
    #[inline(always)]
    fn new(pos: usize) -> Self {
        pos as Self
    }

    #[inline(always)]
    fn position(self, ip: usize) -> usize {
        let back = (ip as u32).wrapping_sub(self).wrapping_sub(1) & (LONG_MAX_DISTANCE as u32 - 1);
        ip - (back as usize + 1)
    }
}

/// Counts of the commands and distances of a meta-block.
trait Tally {
    /// Counts a command by its index into [`CodeTables::implicit`] or
    /// [`CodeTables::explicit`].
    fn command(&mut self, implicit: bool, index: usize);
    fn distance(&mut self, symbol: usize);
}

/// No counts, for fixed codes.
impl Tally for () {
    #[inline(always)]
    fn command(&mut self, _implicit: bool, _index: usize) {}

    #[inline(always)]
    fn distance(&mut self, _symbol: usize) {}
}

/// Command and distance counts, laid out as [`CodeTables`].
#[derive(Clone, Debug)]
struct Counts {
    explicit: [u32; 24 * 32],
    implicit: [u32; 8 * 16],
    distances: [u32; 64],
}

impl Default for Counts {
    fn default() -> Self {
        Self {
            explicit: [0; 24 * 32],
            implicit: [0; 8 * 16],
            distances: [0; 64],
        }
    }
}

/// Counts for a block with no block before: the [`Long`] code, as counts
/// of `2^(15 - len)`.
static INITIAL_COUNTS: Counts = {
    const fn count(len: u8) -> u32 {
        if len == 0 { 0 } else { 1 << (15 - len) }
    }
    let mut counts = Counts {
        explicit: [0; 24 * 32],
        implicit: [0; 8 * 16],
        distances: [0; 64],
    };
    let mut insert_code = 0;
    while insert_code < 24 {
        let mut copy_code = 0;
        while copy_code < 24 {
            counts.explicit[insert_code * 32 + copy_code] =
                count(LONG_COMMAND_LENGTHS[explicit_symbol(insert_code, copy_code)]);
            if insert_code < 8 && copy_code < 16 {
                counts.implicit[insert_code * 16 + copy_code] =
                    count(LONG_COMMAND_LENGTHS[implicit_symbol(insert_code, copy_code)]);
            }
            copy_code += 1;
        }
        insert_code += 1;
    }
    let mut symbol = 0;
    while symbol < 64 {
        counts.distances[symbol] = count(LONG_DISTANCE_LENGTHS[symbol]);
        symbol += 1;
    }
    counts
};

impl Tally for Counts {
    #[inline(always)]
    fn command(&mut self, implicit: bool, index: usize) {
        if implicit {
            self.implicit[index % (8 * 16)] += 1;
        } else {
            self.explicit[index % (24 * 32)] += 1;
        }
    }

    #[inline(always)]
    fn distance(&mut self, symbol: usize) {
        self.distances[symbol % 64] += 1;
    }
}

impl Counts {
    /// Writes command and distance codes for these counts, and returns them.
    /// Every command and distance of a block gets a count of at least one:
    /// the [`Long`] code has codes for every command of an input up to
    /// [`MAX_INPUT`], which takes every length code, and every distance up
    /// to [`LONG_MAX_DISTANCE`] gets one more.
    fn write_codes(&self, writer: &mut BitWriter) -> Result<CodeTables, CompressError> {
        let mut commands = [0_u32; 704];
        for insert_code in 0..24 {
            for copy_code in 0..24 {
                commands[explicit_symbol(insert_code, copy_code)] +=
                    self.explicit[insert_code * 32 + copy_code];
                if insert_code < 8 && copy_code < 16 {
                    commands[implicit_symbol(insert_code, copy_code)] +=
                        self.implicit[insert_code * 16 + copy_code];
                }
            }
        }
        for (count, &len) in commands.iter_mut().zip(&LONG_COMMAND_LENGTHS) {
            *count += u32::from(len != 0);
        }
        let mut distances = self.distances;
        distances[0] += 1;
        for count in &mut distances[16..LONG_DISTANCE_SYMBOLS] {
            *count += 1;
        }
        let (commands, _) = prefix_code::write_tuned(writer, &commands)?;
        let (distances, _) = prefix_code::write_tuned(writer, &distances)?;
        Ok(CodeTables::new(
            &commands.map(table_code),
            distances.map(table_code),
        ))
    }
}

/// A code of [`prefix_code`], `bits << 8 | len`, as in [`CodeTables`].
/// Seeds give codes more than one symbol, so no code is empty.
fn table_code(code: u32) -> u32 {
    let len = code & 0xff;
    debug_assert_ne!(len, 0);
    if len == u32::from(u8::MAX) {
        0
    } else {
        (len << 16) | (code >> 8)
    }
}

/// Writes a literal code for the byte histogram of `input`. Matching takes
/// common bytes out of the literals, so the first 11 of each byte count
/// three times, as in Google's encoder.
fn write_literal_code(writer: &mut BitWriter, input: &[u8]) -> Result<LiteralCodes, CompressError> {
    // Four counters per byte, so runs of one byte do not chain every
    // increment to the one before it.
    let mut counts = [[0_u32; LITERAL_ALPHABET_SIZE]; 4];
    let (quads, rest) = input.as_chunks::<4>();
    for quad in quads {
        for (counts, &byte) in counts.iter_mut().zip(quad) {
            counts[usize::from(byte)] += 1;
        }
    }
    for &byte in rest {
        counts[0][usize::from(byte)] += 1;
    }
    let mut histogram = [0_u32; LITERAL_ALPHABET_SIZE];
    for (symbol, count) in histogram.iter_mut().enumerate() {
        let n = counts[0][symbol] + counts[1][symbol] + counts[2][symbol] + counts[3][symbol];
        *count = n + 2 * n.min(11);
    }
    let (codes, _) = prefix_code::write(writer, &histogram)?;
    Ok(codes)
}

/// [`write_literal_code`] for the histogram of every
/// [`LITERAL_SAMPLE_STRIDE`]th byte of `input`, with each byte value
/// counted once more, so the bytes the sample misses get a code too.
fn write_sampled_literal_code(
    writer: &mut BitWriter,
    input: &[u8],
) -> Result<LiteralCodes, CompressError> {
    let mut counts = [0_u32; LITERAL_ALPHABET_SIZE];
    for &byte in input.iter().step_by(LITERAL_SAMPLE_STRIDE) {
        counts[usize::from(byte)] += 1;
    }
    for count in &mut counts {
        *count += 1 + 2 * (*count).min(11);
    }
    let (codes, _) = prefix_code::write(writer, &counts)?;
    Ok(codes)
}

/// Command output state. It lives in locals of one function, so the sink
/// and the distance ring stay in registers.
struct Commands<'a, 's, T> {
    sink: BitSink<'s>,
    literals: &'a LiteralCodes,
    code: &'a CodeTables,
    ring: DistanceRing,
    tally: &'a mut T,
}

impl<T: Tally> Commands<'_, '_, T> {
    /// Finds matches for `input[start..]` in all of `input`, and writes
    /// `input[start..]` as commands. See [`Entry`] for `table`. Returns
    /// `false` when the output runs out of room.
    #[inline(always)]
    fn write_matches<E: Entry, const TABLE_SIZE: usize>(
        &mut self,
        input: &[u8],
        start: usize,
        table: &mut [E; TABLE_SIZE],
    ) -> bool {
        let len = input.len();
        if len - start <= INPUT_MARGIN + 1 {
            return self.write_insert(&input[start..]);
        }
        let shift = 64 - TABLE_SIZE.trailing_zeros() as usize;
        let ip_limit = len - INPUT_MARGIN;
        let mut next_emit = start;
        // The first byte of an input has nothing to copy from.
        let mut ip = start.max(1);

        'scan: while let Some((found_ip, mut candidate)) =
            find_match(input, ip, ip_limit, self.ring.last(), table)
        {
            ip = found_ip;
            loop {
                let copy_len = MIN_MATCH + match_len(input, candidate + MIN_MATCH, ip + MIN_MATCH);
                if !self.write_command(&input[next_emit..ip], copy_len, ip - candidate) {
                    return false;
                }
                ip += copy_len;
                next_emit = ip;
                if ip >= ip_limit {
                    break 'scan;
                }
                // Hash the last positions of the copy, and look for a match
                // right after it.
                let word = read_u64_le(input, ip - 3);
                table[hash_word_q0(word, shift)] = E::new(ip - 3);
                table[hash_word_q0(word >> 8, shift)] = E::new(ip - 2);
                table[hash_word_q0(word >> 16, shift)] = E::new(ip - 1);
                let key = hash_word_q0(word >> 24, shift);
                candidate = table[key].position(ip);
                table[key] = E::new(ip);
                if !matches5(input, candidate, word >> 24) {
                    break;
                }
            }
            ip += 1;
        }

        next_emit == len || self.write_insert(&input[next_emit..])
    }

    /// Writes `literals`, then a copy of `copy_len` bytes from `distance`
    /// back.
    #[inline(always)]
    fn write_command(&mut self, literals: &[u8], copy_len: usize, distance: usize) -> bool {
        if !self.sink.has_room(COMMAND_MAX_BITS) {
            return false;
        }
        let (insert_code, insert_bits, insert_extra) = insert_code(literals.len());
        let (copy_code, copy_bits, copy_extra) = copy_code(copy_len);
        let last = distance == self.ring.last();
        // Short commands can reuse the last distance without a distance code.
        let implicit = last && insert_code < 8 && copy_code < 16;
        let index = if implicit {
            insert_code * 16 + copy_code
        } else {
            insert_code * 32 + copy_code
        };
        let code = if implicit {
            self.code.implicit[index]
        } else {
            self.code.explicit[index]
        };
        debug_assert_ne!(code, 0);
        self.tally.command(implicit, index);
        let code_bits = (code >> 16) as u8;
        let width = code_bits + insert_bits;
        let bits = u64::from(code & 0xffff) | (insert_extra << code_bits);
        if width + copy_bits <= MAX_BITS_PER_OP {
            self.sink
                .write(width + copy_bits, bits | (copy_extra << width));
        } else {
            self.sink.write(width, bits);
            self.sink.write(copy_bits, copy_extra);
        }

        if !self.write_literals(literals) {
            return false;
        }
        if implicit {
            return true;
        }
        if !self.sink.has_room(DISTANCE_MAX_BITS) {
            return false;
        }
        if last {
            // Distance symbol 0 repeats the last distance.
            self.tally.distance(0);
            let code = self.code.distances[0];
            self.sink
                .write((code >> 16) as u8, u64::from(code & 0xffff));
            return true;
        }
        let (symbol, extra_bits, extra) = distance_code(distance);
        self.tally.distance(symbol);
        let code = self.code.distances[symbol];
        let code_bits = (code >> 16) as u8;
        self.sink.write(
            code_bits + extra_bits,
            u64::from(code & 0xffff) | (extra << code_bits),
        );
        self.ring.push(distance);
        true
    }

    /// Writes an insert-only command for `literals`, which end the input.
    #[inline(always)]
    fn write_insert(&mut self, literals: &[u8]) -> bool {
        if !self.sink.has_room(COMMAND_MAX_BITS) {
            return false;
        }
        let (insert_code, insert_bits, insert_extra) = insert_code(literals.len());
        // The meta-block ends after the literals, so the copy length is
        // never used and no distance follows.
        let (implicit, index, code) = if insert_code < 8 {
            (true, insert_code * 16, self.code.implicit[insert_code * 16])
        } else {
            (
                false,
                insert_code * 32,
                self.code.explicit[insert_code * 32],
            )
        };
        debug_assert_ne!(code, 0);
        self.tally.command(implicit, index);
        let code_bits = (code >> 16) as u8;
        self.sink.write(
            code_bits + insert_bits,
            u64::from(code & 0xffff) | (insert_extra << code_bits),
        );
        self.write_literals(literals)
    }

    #[inline(always)]
    fn write_literals(&mut self, literals: &[u8]) -> bool {
        let codes = self.literals;
        for run in literals.chunks(LITERAL_RUN) {
            if !self.sink.has_room(run.len() * usize::from(FAST_CODE_BITS)) {
                return false;
            }
            // Codes are at most 14 bits, so four fit one write.
            let (quads, rest) = run.as_chunks::<4>();
            for quad in quads {
                let mut bits = 0_u64;
                let mut width = 0_u8;
                for &byte in quad {
                    let code = codes[usize::from(byte)];
                    bits |= u64::from(code >> 8) << width;
                    width += code as u8;
                }
                self.sink.write(width, bits);
            }
            for &byte in rest {
                let code = codes[usize::from(byte)];
                self.sink.write(code as u8, u64::from(code >> 8));
            }
        }
        true
    }
}

/// Scans from `ip` for a match of 5 bytes that starts by `ip_limit`, first
/// at `last_distance`, then at the position from `table`, which takes `ip`.
/// After 32 misses it looks at every other position, after 32 more at
/// every third, and so on, as Google's encoder does. Returns where the
/// match starts and the earlier position it matches.
#[inline(always)]
fn find_match<E: Entry, const TABLE_SIZE: usize>(
    input: &[u8],
    ip: usize,
    ip_limit: usize,
    last_distance: usize,
    table: &mut [E; TABLE_SIZE],
) -> Option<(usize, usize)> {
    let shift = 64 - TABLE_SIZE.trailing_zeros() as usize;
    let mut skip = 32;
    let mut next_ip = ip;
    let mut next_word = read_u64_le(input, ip);
    loop {
        let word = next_word;
        let ip = next_ip;
        next_ip = ip + (skip >> 5);
        skip += 1;
        if next_ip > ip_limit {
            return None;
        }
        next_word = read_u64_le(input, next_ip);
        let key = hash_word_q0(word, shift);
        if let Some(last) = ip.checked_sub(last_distance)
            && matches5(input, last, word)
        {
            table[key] = E::new(ip);
            return Some((ip, last));
        }
        let candidate = table[key].position(ip);
        table[key] = E::new(ip);
        if matches5(input, candidate, word) {
            return Some((ip, candidate));
        }
    }
}

/// Length of the match of `input[pos..]` with `input[candidate..]`, for
/// `candidate < pos`.
#[inline(always)]
pub(super) fn match_len(input: &[u8], candidate: usize, pos: usize) -> usize {
    let tail = &input[pos..];
    let earlier = &input[candidate..candidate + tail.len()];
    // Words of 8 bytes, which need no bounds checks.
    let (words, _) = tail.as_chunks::<8>();
    let (earlier_words, _) = earlier.as_chunks::<8>();
    for (index, (&word, &earlier_word)) in words.iter().zip(earlier_words).enumerate() {
        let diff = u64::from_le_bytes(word) ^ u64::from_le_bytes(earlier_word);
        if diff != 0 {
            return index * 8 + diff.trailing_zeros() as usize / 8;
        }
    }
    let mut len = words.len() * 8;
    while len < tail.len() && earlier[len] == tail[len] {
        len += 1;
    }
    len
}

/// Whether the 5 bytes at `pos` match the low 5 bytes of `word`. `pos`
/// must leave 8 bytes to read.
#[inline(always)]
fn matches5(input: &[u8], pos: usize, word: u64) -> bool {
    (load64(input, pos) ^ word) << 24 == 0
}

/// The 8 bytes at `pos`, which is below 2^32 on this path. Saying so lets
/// the bounds check skip the overflow test.
#[inline(always)]
pub(super) fn load64(input: &[u8], pos: usize) -> u64 {
    debug_assert!(u32::try_from(pos).is_ok());
    let pos = pos as u32 as usize;
    u64::from_le_bytes(input[pos..pos + 8].try_into().unwrap())
}

/// Insert length codes below 130, each `base << 16 | extra_bits << 8 | code`.
static INSERT_CODES: [u32; 130] = {
    let mut table = [0; 130];
    let mut len = 0;
    while len < 130 {
        let (code, bits, extra) = insert_code_slow(len);
        table[len] = ((len as u32 - extra as u32) << 16) | ((bits as u32) << 8) | code as u32;
        len += 1;
    }
    table
};

/// Copy length codes below 134, packed as in [`INSERT_CODES`].
static COPY_CODES: [u32; 134] = {
    let mut table = [0; 134];
    let mut len = 2;
    while len < 134 {
        let (code, bits, extra) = copy_code_slow(len);
        table[len] = ((len as u32 - extra as u32) << 16) | ((bits as u32) << 8) | code as u32;
        len += 1;
    }
    table
};

/// Insert length code, extra bit count, and extra bits for `len`.
#[inline(always)]
pub(super) fn insert_code(len: usize) -> (usize, u8, u64) {
    match INSERT_CODES.get(len) {
        Some(&entry) => unpack_length_code(entry, len),
        None => insert_code_slow(len),
    }
}

/// Copy length code, extra bit count, and extra bits for `len` of at least
/// 2.
#[inline(always)]
pub(super) fn copy_code(len: usize) -> (usize, u8, u64) {
    match COPY_CODES.get(len) {
        Some(&entry) => unpack_length_code(entry, len),
        None => copy_code_slow(len),
    }
}

#[inline(always)]
fn unpack_length_code(entry: u32, len: usize) -> (usize, u8, u64) {
    (
        (entry & 0xff) as usize,
        (entry >> 8) as u8,
        (len - (entry >> 16) as usize) as u64,
    )
}

const fn insert_code_slow(len: usize) -> (usize, u8, u64) {
    if len < 6 {
        (len, 0, 0)
    } else if len < 130 {
        let tail = len - 2;
        let bits = tail.ilog2() as usize - 1;
        let prefix = tail >> bits;
        (
            2 * bits + prefix + 2,
            bits as u8,
            (tail - (prefix << bits)) as u64,
        )
    } else if len < 2114 {
        let tail = len - 66;
        let bits = tail.ilog2() as usize;
        (bits + 10, bits as u8, (tail - (1 << bits)) as u64)
    } else if len < 6210 {
        (21, 12, (len - 2114) as u64)
    } else if len < 22594 {
        (22, 14, (len - 6210) as u64)
    } else {
        (23, 24, (len - 22594) as u64)
    }
}

const fn copy_code_slow(len: usize) -> (usize, u8, u64) {
    if len < 10 {
        (len - 2, 0, 0)
    } else if len < 134 {
        let tail = len - 6;
        let bits = tail.ilog2() as usize - 1;
        let prefix = tail >> bits;
        (
            2 * bits + prefix + 4,
            bits as u8,
            (tail - (prefix << bits)) as u64,
        )
    } else if len < 2118 {
        let tail = len - 70;
        let bits = tail.ilog2() as usize;
        (bits + 12, bits as u8, (tail - (1 << bits)) as u64)
    } else {
        (23, 24, (len - 2118) as u64)
    }
}

/// Distance symbol, extra bit count, and extra bits for `distance` without
/// postfix bits or direct codes.
#[inline(always)]
pub(super) fn distance_code(distance: usize) -> (usize, u8, u64) {
    let d = distance + 3;
    let bits = d.ilog2() as usize - 1;
    let prefix = (d >> bits) & 1;
    (
        16 + 2 * (bits - 1) + prefix,
        bits as u8,
        (d - ((2 + prefix) << bits)) as u64,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn length_codes_match_general_codes() {
        for len in 0..30_000 {
            let general = super::super::insert_length_code(len).unwrap();
            let (code, bits, extra) = insert_code(len);
            assert_eq!(
                (code, bits, extra),
                (general.code, general.extra_bits, u64::from(general.extra)),
                "insert {len}"
            );
        }
        for len in 2..30_000 {
            let general = super::super::copy_length_code(len).unwrap();
            let (code, bits, extra) = copy_code(len);
            assert_eq!(
                (code, bits, extra),
                (general.code, general.extra_bits, u64::from(general.extra)),
                "copy {len}"
            );
        }
        for distance in 1..100_000 {
            let general = super::super::distance_code(distance).unwrap();
            let (symbol, bits, extra) = distance_code(distance);
            assert_eq!(
                (symbol, bits, extra),
                (
                    usize::from(general.symbol),
                    general.extra_bits,
                    u64::from(general.extra)
                ),
                "distance {distance}"
            );
        }
    }

    /// `len` random bytes, or with `skew` bytes from 128 up, each the AND of
    /// two random ones, with about 5.7 bits of entropy. Neither repeats much.
    fn random_bytes(len: usize, skew: bool) -> Vec<u8> {
        let mut state = 0x9e37_79b9_7f4a_7c15_u64;
        (0..len)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                if skew {
                    0x80 | (state as u8 & (state >> 8) as u8)
                } else {
                    state as u8
                }
            })
            .collect()
    }

    #[test]
    fn low_compressibility_blocks_take_literal_codes() {
        let skewed = random_bytes(3 * LONG_BLOCK + 1000, true);
        let mut writer = BitWriter::new();
        write_long(&mut writer, &skewed, &mut DistanceRing::default()).unwrap();
        // About 6.7 bits per byte, with the high bit.
        assert!(writer.written_bits() < skewed.len() * 7);

        // Uniform bytes take no code, and their blocks go out stored.
        let uniform = random_bytes(2 * LONG_BLOCK, false);
        let mut writer = BitWriter::new();
        write_long(&mut writer, &uniform, &mut DistanceRing::default()).unwrap();
        assert!(writer.written_bits() >= uniform.len() * 8);

        for input in [&skewed, &uniform] {
            let encoded = crate::compress(input, 0).unwrap();
            assert_eq!(burli_decode::decompress(&encoded).unwrap(), *input);
        }
    }

    #[test]
    fn long_blocks_have_codes() {
        // Seeded with the Long code, adapted codes cover every command a
        // block of any length can write.
        let mut writer = BitWriter::new();
        let code = Counts::default().write_codes(&mut writer).unwrap();
        for insert_code in 0..24 {
            let only = if insert_code < 8 {
                code.implicit[insert_code * 16]
            } else {
                code.explicit[insert_code * 32]
            };
            assert_ne!(only, 0, "insert {insert_code}");
            for copy_code in copy_code(MIN_MATCH).0..24 {
                assert_ne!(code.explicit[insert_code * 32 + copy_code], 0);
                if insert_code < 8 && copy_code < 16 {
                    assert_ne!(code.implicit[insert_code * 16 + copy_code], 0);
                }
            }
        }
        assert_ne!(code.distances[0], 0);
        for distance in [1, 4096, 65_536, 100_000, LONG_MAX_DISTANCE] {
            assert_ne!(code.distances[distance_code(distance).0], 0, "{distance}");
        }
    }

    #[test]
    fn reachable_symbols_have_codes() {
        // Each code by the largest input it takes.
        for (max, code) in [
            (4096, Flat::CODE),
            (16384, Short::CODE),
            (MAX_INPUT, Long::CODE),
        ] {
            for insert in 0..=max {
                let (insert_code, _, _) = insert_code(insert);
                let only = if insert_code < 8 {
                    code.tables.implicit[insert_code * 16]
                } else {
                    code.tables.explicit[insert_code * 32]
                };
                assert_ne!(only, 0, "max {max} insert {insert}");
            }
            for insert_code in 0..=insert_code(max).0 {
                for copy in MIN_MATCH..max {
                    let (copy_code, _, _) = copy_code(copy);
                    assert_ne!(
                        code.tables.explicit[insert_code * 32 + copy_code],
                        0,
                        "max {max}"
                    );
                    if insert_code < 8 && copy_code < 16 {
                        assert_ne!(
                            code.tables.implicit[insert_code * 16 + copy_code],
                            0,
                            "max {max}"
                        );
                    }
                }
            }
            assert_ne!(code.tables.distances[0], 0);
            for distance in 1..max {
                assert_ne!(
                    code.tables.distances[distance_code(distance).0],
                    0,
                    "max {max} distance {distance}"
                );
            }
        }
    }
}
