//! Two-pass q1 encoder for short inputs.
//!
//! The q1 matcher collects commands with Brotli's combined insert-and-copy
//! symbols, and counts literals, commands, and distances as it goes. Then
//! each gets a Huffman code from its counts, and the commands go out with
//! them. A Huffman code per input takes fewer bits than the fixed codes of
//! the one-pass q0 encoder, and combined symbols fewer than the split
//! insert and copy symbols of the q1 batch for longer inputs.

use alloc::vec::Vec;

use burli_core::{
    CompressError,
    bits::{BitSink, BitWriter, MAX_BITS_PER_OP},
};

use super::{
    COMMAND_ALPHABET_SIZE, DistanceRing, LITERAL_ALPHABET_SIZE,
    fixed_codes::COMMAND_CELLS,
    one_pass::{copy_code, distance_code, insert_code},
    prefix_code::{self, Codes, LiteralCodes},
    q1, write_block_and_context_header, write_meta_block_len,
};

/// Longest input this path takes. Lengths and distances then fit the
/// fields of [`Command`].
pub(super) const MAX_INPUT: usize = 1 << 16;
const _: () = assert!(super::tune::Q1_TWO_PASS_MAX_INPUT <= MAX_INPUT);

const DISTANCE_ALPHABET_SIZE: usize = 64;

/// Command symbols below this reuse the last distance without a distance
/// code.
const IMPLICIT_SYMBOLS: usize = 128;

/// [`COMMAND_CELLS`] in 4 bits each, so a lookup needs no table in memory.
const PACKED_CELLS: u64 = {
    let mut packed = 0;
    let mut index = 0;
    while index < COMMAND_CELLS.len() {
        packed |= (COMMAND_CELLS[index] as u64) << (4 * index);
        index += 1;
    }
    packed
};

/// The command cell for an insert and copy length code.
#[inline(always)]
const fn command_cell(insert_code: usize, copy_code: usize) -> usize {
    ((PACKED_CELLS >> (4 * ((insert_code >> 3) * 3 + (copy_code >> 3)))) & 15) as usize
}

/// A copy command as it goes out.
#[derive(Clone, Copy, Debug)]
struct Command {
    /// The command symbol, the width of its extra bits from bit 10, and its
    /// insert and then copy extra bits from bit 16.
    head: u64,
    /// The insert length, the copy length from bit 17, and from bit 34 the
    /// distance code as `extra << 11 | width << 6 | symbol`, for commands
    /// that have one.
    tail: u64,
}

/// The commands of one input, with the counts of their symbols.
#[derive(Clone, Debug)]
pub(super) struct Commands {
    copies: Vec<Command>,
    /// The insert-only command for the literals after the last copy, as in
    /// [`Command::head`], and their count.
    tail: (u64, u32),
    literal_counts: [u32; LITERAL_ALPHABET_SIZE],
    command_counts: [u32; COMMAND_ALPHABET_SIZE],
    distance_counts: [u32; DISTANCE_ALPHABET_SIZE],
    /// Extra bits of all commands and distances.
    extra_bits: usize,
    ring: DistanceRing,
}

impl Default for Commands {
    fn default() -> Self {
        Self {
            copies: Vec::new(),
            tail: (0, 0),
            literal_counts: [0; LITERAL_ALPHABET_SIZE],
            command_counts: [0; COMMAND_ALPHABET_SIZE],
            distance_counts: [0; DISTANCE_ALPHABET_SIZE],
            extra_bits: 0,
            ring: DistanceRing::default(),
        }
    }
}

/// Collects and writes `input` as compressed meta-blocks, and advances
/// `ring` past their copies. A meta-block takes at most `max_distance` and
/// [`q1::INPUT_MARGIN_BYTES`] bytes. The matcher stops that many bytes
/// before the end, so no copy reaches past the window.
pub(super) fn write(
    writer: &mut BitWriter,
    input: &[u8],
    max_distance: usize,
    ring: &mut DistanceRing,
    q1: &mut q1::Workspace,
    commands: &mut Commands,
) -> Result<(), CompressError> {
    debug_assert!(!input.is_empty() && input.len() <= MAX_INPUT);
    for piece in input.chunks(max_distance + q1::INPUT_MARGIN_BYTES) {
        commands.reset(piece.len(), *ring);
        q1::collect_short(piece, max_distance, q1, commands);
        commands.write(writer, piece)?;
        *ring = commands.ring;
    }
    Ok(())
}

impl Commands {
    fn reset(&mut self, input_len: usize, ring: DistanceRing) {
        self.copies.clear();
        self.copies.reserve(input_len / 16);
        self.tail = (0, 0);
        self.literal_counts.fill(0);
        self.command_counts.fill(0);
        self.distance_counts.fill(0);
        self.extra_bits = 0;
        self.ring = ring;
    }

    #[inline(always)]
    fn count_literals(&mut self, literals: &[u8]) {
        for &literal in literals {
            self.literal_counts[usize::from(literal)] += 1;
        }
    }

    fn write(&self, writer: &mut BitWriter, input: &[u8]) -> Result<(), CompressError> {
        write_meta_block_len(writer, input.len())?;
        write_block_and_context_header(writer)?;
        let (literals, literal_bits) = prefix_code::write_tuned(writer, &self.literal_counts)?;
        let (codes, command_bits) = prefix_code::write_tuned(writer, &self.command_counts)?;
        let (distances, distance_bits) = prefix_code::write_tuned(writer, &self.distance_counts)?;
        let total_bits = literal_bits + command_bits + distance_bits + self.extra_bits;
        writer.write_bits_with(total_bits, |sink| {
            let mut pos = 0;
            for command in &self.copies {
                write_command(sink, &codes, command.head);
                let insert_len = (command.tail & 0x1_ffff) as usize;
                write_literals(sink, &literals, &input[pos..pos + insert_len]);
                pos += insert_len + ((command.tail >> 17) & 0x1_ffff) as usize;
                if (command.head & 0x3ff) as usize >= IMPLICIT_SYMBOLS {
                    let distance = command.tail >> 34;
                    let code = distances[(distance & 0x3f) as usize];
                    let len = code as u8;
                    sink.write(
                        len + ((distance >> 6) & 0x1f) as u8,
                        u64::from(code >> 8) | ((distance >> 11) << len),
                    );
                }
            }
            let (head, tail_len) = self.tail;
            if tail_len != 0 {
                write_command(sink, &codes, head);
                write_literals(sink, &literals, &input[pos..]);
            }
        });
        Ok(())
    }
}

impl q1::Sink for Commands {
    #[inline(always)]
    fn push_literals(&mut self, input: &[u8], insert_start: usize, insert_len: usize) {
        // Collectors push literals only once, after the last copy.
        debug_assert_eq!(self.tail.1, 0);
        if insert_len == 0 {
            return;
        }
        self.count_literals(&input[insert_start..insert_start + insert_len]);
        let (insert_code, insert_bits, insert_extra) = insert_code(insert_len);
        // The meta-block ends after the literals, so the copy length is
        // never used and no distance follows.
        let symbol = if insert_code < 8 {
            insert_code * 8
        } else {
            command_cell(insert_code, 0) * 64 + (insert_code & 7) * 8
        };
        self.command_counts[symbol] += 1;
        self.extra_bits += usize::from(insert_bits);
        self.tail = (
            symbol as u64 | (u64::from(insert_bits) << 10) | (insert_extra << 16),
            insert_len as u32,
        );
    }

    #[inline(always)]
    fn push_copy(
        &mut self,
        input: &[u8],
        insert_start: usize,
        insert_len: usize,
        copy_len: usize,
        distance: usize,
        _last_distance: usize,
    ) {
        self.count_literals(&input[insert_start..insert_start + insert_len]);
        let (insert_code, insert_bits, insert_extra) = insert_code(insert_len);
        let (copy_code, copy_bits, copy_extra) = copy_code(copy_len);
        let low = (insert_code & 7) * 8 + (copy_code & 7);
        let last = distance == self.ring.last();
        let mut tail = insert_len as u64 | ((copy_len as u64) << 17);
        let symbol = if last && insert_code < 8 && copy_code < 16 {
            (copy_code >> 3) * 64 + low
        } else {
            // Distance symbol 0 repeats the last distance.
            let (symbol, bits, extra) = if last {
                (0, 0, 0)
            } else {
                self.ring.push(distance);
                distance_code(distance)
            };
            self.distance_counts[symbol] += 1;
            self.extra_bits += usize::from(bits);
            tail |= ((extra << 11) | (u64::from(bits) << 6) | symbol as u64) << 34;
            command_cell(insert_code, copy_code) * 64 + low
        };
        self.command_counts[symbol] += 1;
        let width = insert_bits + copy_bits;
        self.extra_bits += usize::from(width);
        self.copies.push(Command {
            head: symbol as u64
                | (u64::from(width) << 10)
                | ((insert_extra | (copy_extra << insert_bits)) << 16),
            tail,
        });
    }
}

/// Writes the command code of `head` with its extra bits.
#[inline(always)]
pub(super) fn write_command(
    sink: &mut BitSink<'_>,
    codes: &Codes<COMMAND_ALPHABET_SIZE>,
    head: u64,
) {
    let code = codes[(head & 0x3ff) as usize];
    let len = code as u8;
    let width = ((head >> 10) & 0x3f) as u8;
    let extra = head >> 16;
    if len + width <= MAX_BITS_PER_OP {
        sink.write(len + width, u64::from(code >> 8) | (extra << len));
    } else {
        sink.write(len, u64::from(code >> 8));
        sink.write(width, extra);
    }
}

#[inline(always)]
pub(super) fn write_literals(sink: &mut BitSink<'_>, codes: &LiteralCodes, literals: &[u8]) {
    // Codes are at most 14 bits, so four fit one write.
    let (quads, rest) = literals.as_chunks::<4>();
    for quad in quads {
        let mut bits = 0_u64;
        let mut width = 0_u8;
        for &literal in quad {
            let code = codes[usize::from(literal)];
            bits |= u64::from(code >> 8) << width;
            width += code as u8;
        }
        sink.write(width, bits);
    }
    for &literal in rest {
        let code = codes[usize::from(literal)];
        sink.write(code as u8, u64::from(code >> 8));
    }
}
