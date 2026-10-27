//! Two-pass q1 encoder.
//!
//! For short inputs, the q1 matcher collects commands with Brotli's
//! combined insert-and-copy symbols, and counts literals, commands, and
//! distances as it goes. Then each gets a Huffman code from its counts, and
//! the commands go out with them. A Huffman code per input takes fewer bits
//! than the fixed codes of the one-pass q0 encoder. Longer inputs take
//! [`collect_long`], whose commands go to the token writer of
//! [`super::tokens`], which codes them the same way per meta-block.

use alloc::vec::Vec;

use burli_core::{
    CompressError,
    bits::{BitSink, BitWriter, MAX_BITS_PER_OP},
};

use super::{
    COMMAND_ALPHABET_SIZE, DistanceRing, LITERAL_ALPHABET_SIZE, Token,
    fixed_codes::COMMAND_CELLS,
    one_pass::{copy_code, distance_code, insert_code, load64, match_len},
    prefix_code::{self, Codes, LiteralCodes},
    q1,
    tokens::Sink,
    write_block_and_context_header, write_meta_block_len,
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

/// Collects the commands of long `input` into `sink` as tokens, with a hash
/// table of `TABLE_SIZE` positions and matches of at least `MIN_MATCH`
/// bytes. After a copy, its last `TAIL` positions go into the table.
/// Copies reach back at most `max_distance`. The first copy can reuse
/// `last_distance`, the decoder's last distance.
pub(super) fn collect_long<const TABLE_SIZE: usize, const MIN_MATCH: usize, const TAIL: usize>(
    input: &[u8],
    max_distance: usize,
    last_distance: usize,
    table: &mut Vec<u32>,
    sink: &mut impl Sink,
) {
    // A copy ends at least `MIN_MATCH + 1` bytes in.
    const { assert!(TAIL <= MIN_MATCH + 1) };
    debug_assert_ne!(last_distance, 0);
    let len = input.len();
    if len <= LONG_INPUT_MARGIN + 1 {
        sink.push_tail(0, len);
        return;
    }
    table.clear();
    table.resize(TABLE_SIZE, 0);
    let table: &mut [u32; TABLE_SIZE] = table.as_mut_slice().try_into().unwrap();
    let ip_limit = len - LONG_INPUT_MARGIN;
    let mut last_distance = last_distance;
    let mut next_emit = 0;
    let mut ip = 1;

    'scan: while let Some((found_ip, mut candidate)) = find_long_match::<TABLE_SIZE, MIN_MATCH>(
        input,
        ip,
        ip_limit,
        last_distance,
        max_distance,
        table,
    ) {
        ip = found_ip;
        loop {
            let distance = ip - candidate;
            let copy_len = MIN_MATCH + match_len(input, candidate + MIN_MATCH, ip + MIN_MATCH);
            sink.push(Token {
                insert_start: next_emit,
                insert_len: ip - next_emit,
                copy_len,
                copy_len_code: 0,
                distance,
                distance_code: None,
                use_last_distance: distance == last_distance,
            });
            last_distance = distance;
            ip += copy_len;
            next_emit = ip;
            if ip >= ip_limit {
                break 'scan;
            }
            // Hash the last positions of the copy, and look for a match
            // right after it.
            for pos in ip - TAIL..ip {
                table[long_hash::<TABLE_SIZE, MIN_MATCH>(load64(input, pos))] = pos as u32;
            }
            let word = load64(input, ip);
            let key = long_hash::<TABLE_SIZE, MIN_MATCH>(word);
            candidate = table[key] as usize;
            table[key] = ip as u32;
            if ip - candidate > max_distance || !long_matches::<MIN_MATCH>(input, candidate, word) {
                break;
            }
        }
        ip += 1;
    }
    sink.push_tail(next_emit, len);
}

/// [`collect_long`] stops matching this many bytes before the end, so every
/// load reads 8 bytes in bounds.
const LONG_INPUT_MARGIN: usize = 8;

/// Samples [`looks_binary`] takes.
const BINARY_SAMPLES: usize = 4096;

/// Whether more than 1 in 12 of [`BINARY_SAMPLES`] bytes spread over `input`
/// are zero, as in binary code and data. Text has almost none.
pub(super) fn looks_binary(input: &[u8]) -> bool {
    let step = (input.len() / BINARY_SAMPLES).max(1);
    let zeros = input
        .iter()
        .step_by(step)
        .filter(|&&byte| byte == 0)
        .count();
    zeros * 12 > input.len().div_ceil(step)
}

/// Scans from `ip` for a match that starts by `ip_limit`, first at
/// `last_distance`, then at the position from `table`, which takes `ip`.
/// After 32 misses it looks at every other position, after 32 more at
/// every third, and so on. Returns where the match starts and the earlier
/// position it matches.
#[inline(always)]
fn find_long_match<const TABLE_SIZE: usize, const MIN_MATCH: usize>(
    input: &[u8],
    ip: usize,
    ip_limit: usize,
    last_distance: usize,
    max_distance: usize,
    table: &mut [u32; TABLE_SIZE],
) -> Option<(usize, usize)> {
    let mut skip = 32;
    let mut next_ip = ip;
    let mut next_word = load64(input, ip);
    loop {
        let word = next_word;
        let ip = next_ip;
        next_ip = ip + (skip >> 5);
        skip += 1;
        if next_ip > ip_limit {
            return None;
        }
        next_word = load64(input, next_ip);
        let key = long_hash::<TABLE_SIZE, MIN_MATCH>(word);
        if let Some(last) = ip.checked_sub(last_distance)
            && long_matches::<MIN_MATCH>(input, last, word)
        {
            table[key] = ip as u32;
            return Some((ip, last));
        }
        // Entries hold earlier positions only.
        let candidate = table[key] as usize;
        table[key] = ip as u32;
        if ip - candidate <= max_distance && long_matches::<MIN_MATCH>(input, candidate, word) {
            return Some((ip, candidate));
        }
    }
}

/// The hash of the low `MIN_MATCH` bytes of `word`, as Google's encoder
/// takes it.
#[inline(always)]
fn long_hash<const TABLE_SIZE: usize, const MIN_MATCH: usize>(word: u64) -> usize {
    let shift = 64 - TABLE_SIZE.trailing_zeros();
    ((word << (64 - 8 * MIN_MATCH)).wrapping_mul(0x1e35_a7bd) >> shift) as usize
}

/// Whether the `MIN_MATCH` bytes at `pos` match the low bytes of `word`.
#[inline(always)]
fn long_matches<const MIN_MATCH: usize>(input: &[u8], pos: usize, word: u64) -> bool {
    (load64(input, pos) ^ word) << (64 - 8 * MIN_MATCH) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Checks that `tokens` cover `input` in order, and that every copy
    /// repeats bytes at most `max_distance` back. `last` is the last
    /// distance before them.
    fn check_tokens(
        input: &[u8],
        tokens: &[Token],
        min_match: usize,
        max_distance: usize,
        mut last: usize,
    ) {
        let mut pos = 0;
        for token in tokens {
            assert_eq!(token.insert_start, pos);
            pos += token.insert_len;
            if token.is_copy() {
                assert!(token.copy_len >= min_match);
                assert!(token.distance <= max_distance.min(pos));
                assert_eq!(token.use_last_distance, last == token.distance);
                for offset in 0..token.copy_len {
                    assert_eq!(input[pos + offset], input[pos + offset - token.distance]);
                }
                last = token.distance;
                pos += token.copy_len;
            }
        }
        assert_eq!(pos, input.len());
    }

    #[test]
    fn long_collector_covers_input() {
        let text = b"the quick brown fox jumps over the lazy dog, again and again. ".repeat(4000);
        let mut binary = Vec::new();
        for index in 0_u32..60_000 {
            binary.extend_from_slice(&[0, (index % 7) as u8, 0, (index % 251) as u8]);
        }
        // A copy from 2 back ends where a string from 216 back starts.
        let q = b"ghijklmnopqrstuv";
        let far = [&q[..], &b"xy".repeat(100), q, &[0; 16]].concat();
        let mut table = Vec::new();
        for input in [&text[..], &binary[..], &far[..], &text[..20], &text[..9]] {
            for max_distance in [100, 1 << 18] {
                let mut tokens = Vec::new();
                collect_long::<{ 1 << 16 }, 6, 4>(input, max_distance, 4, &mut table, &mut tokens);
                check_tokens(input, &tokens, 6, max_distance, 4);
                let mut tokens = Vec::new();
                collect_long::<{ 1 << 15 }, 5, 5>(input, max_distance, 62, &mut table, &mut tokens);
                check_tokens(input, &tokens, 5, max_distance, 62);
            }
        }
        assert!(looks_binary(&binary));
        assert!(!looks_binary(&text));
    }

    #[test]
    fn long_collector_reuses_last_distance() {
        // The decoder's last distance, 4, repeats right away.
        let input = b"abcdabcdabcdabcdabcdabcdabcdabcdabcdabcd";
        let mut tokens = Vec::new();
        collect_long::<{ 1 << 15 }, 5, 5>(input, 1 << 18, 4, &mut Vec::new(), &mut tokens);
        assert_eq!(tokens[0].distance, 4);
        assert!(tokens[0].use_last_distance);
    }
}
