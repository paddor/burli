//! Writes the tokens of the q1 to q5 collectors as compressed meta-blocks.
//!
//! Tokens stream in from a collector, so no list of them is kept. Each one
//! is packed into a [`Command`] with its symbols and extra bits, and its
//! literals, command, and distance get counted. Once a meta-block holds
//! enough symbols, each alphabet gets a Huffman code from its counts, and
//! the commands go out in one pass through a [`BitSink`] sized from those
//! counts.

use alloc::vec::Vec;

use burli_core::{BurliError, CompressError, bits::BitWriter};

use super::{
    COMMAND_ALPHABET_SIZE, DistanceRing, FAST_CODE_BITS, LITERAL_ALPHABET_SIZE,
    MAX_META_BLOCK_SIZE, PrefixCodeScratch, Token,
    fixed_codes::{explicit_symbol, implicit_symbol},
    has_distance_code,
    one_pass::{copy_code, distance_code, insert_code},
    prefix_code::{self, Codes},
    q2_tiny_balanced_literal_prefix_is_likely_safe, reverse_bits_u16, two_pass,
    write_balanced_fast_dense_prefix_code_array_from_frequencies_with_scratch,
    write_block_and_context_header, write_compressed_literal_meta_block,
    write_fast_dense_prefix_code_array_from_frequencies_with_scratch, write_meta_block_len,
    write_static_command_and_distance_prefix_codes,
};

const DISTANCE_ALPHABET_SIZE: usize = 64;

/// [`Command::distance`] of commands without a distance code.
const NO_DISTANCE: u64 = u64::MAX;

/// Counts of up to this get a prefix code from them as they are.
const MAX_COUNT: u32 = 1 << 22;

/// Meta-blocks of up to this many bytes with static command codes get a
/// literal code of at most [`super::FAST_CODE_BITS`] bits.
const MAX_FAST_LITERAL_BLOCK: usize = 1024;

/// Command codes of the static code that
/// [`write_static_command_and_distance_prefix_codes`] writes, as in
/// [`Codes`]: 9 bits for the first 448 symbols and 11 for the rest.
static STATIC_COMMAND_CODES: Codes<COMMAND_ALPHABET_SIZE> = {
    let mut codes = [0; COMMAND_ALPHABET_SIZE];
    let mut symbol = 0;
    while symbol < COMMAND_ALPHABET_SIZE {
        let (len, code) = if symbol < 448 {
            (9, symbol)
        } else {
            (11, 1792 + symbol - 448)
        };
        codes[symbol] = ((reverse_bits_u16(code as u16, len) as u32) << 8) | len as u32;
        symbol += 1;
    }
    codes
};

/// Distance codes of the static code, 6 bits each.
static STATIC_DISTANCE_CODES: Codes<DISTANCE_ALPHABET_SIZE> = {
    let mut codes = [0; DISTANCE_ALPHABET_SIZE];
    let mut symbol = 0;
    while symbol < DISTANCE_ALPHABET_SIZE {
        codes[symbol] = ((reverse_bits_u16(symbol as u16, 6) as u32) << 8) | 6;
        symbol += 1;
    }
    codes
};

/// Where the q1 to q5 collectors put their tokens, in input order.
pub(super) trait Sink {
    fn push(&mut self, token: Token);

    /// Pushes the literals from `insert_start` up to `end` as the last
    /// token, if there are any.
    fn push_tail(&mut self, insert_start: usize, end: usize) {
        if insert_start < end {
            self.push(Token {
                insert_start,
                insert_len: end - insert_start,
                copy_len: 0,
                copy_len_code: 0,
                distance: 0,
                distance_code: None,
                use_last_distance: false,
            });
        }
    }
}

impl Sink for Vec<Token> {
    #[inline(always)]
    fn push(&mut self, token: Token) {
        Vec::push(self, token);
    }
}

/// A token as it goes out.
#[derive(Clone, Copy, Debug)]
struct Command {
    /// The command symbol, the width of its extra bits from bit 10, and its
    /// insert and then copy extra bits from bit 16.
    head: u64,
    /// The distance code as `extra << 11 | width << 6 | symbol`, or
    /// [`NO_DISTANCE`].
    distance: u64,
    insert_start: u32,
    insert_len: u32,
}

/// Writes the tokens of one input as they come, in meta-blocks that end
/// once they hold a symbol limit of literals and commands.
pub(super) struct Writer<'a> {
    output: &'a mut BitWriter,
    input: &'a [u8],
    input_base: usize,
    max_backward_distance: usize,
    symbol_limit: usize,
    /// Meta-blocks of at most this many commands get the static command and
    /// distance codes.
    max_static_commands: usize,
    commands: Vec<Command>,
    literal_counts: [u32; LITERAL_ALPHABET_SIZE],
    command_counts: [u32; COMMAND_ALPHABET_SIZE],
    distance_counts: [u32; DISTANCE_ALPHABET_SIZE],
    /// Extra bits of the commands and distances so far.
    extra_bits: usize,
    /// Literals and commands so far.
    symbols: usize,
    block_len: usize,
    has_copy: bool,
    /// The last four distances that advance the decoder's ring, at
    /// `ring_len % 4` and before.
    ring: [usize; 4],
    ring_len: usize,
    result: Result<(), CompressError>,
}

impl<'a> Writer<'a> {
    /// A writer for the tokens of `input`, which starts `input_base` bytes
    /// into the stream. Meta-blocks end once they hold `symbol_limit`
    /// symbols, and those of at most `max_static_commands` commands get the
    /// static command and distance codes.
    pub(super) fn new(
        writer: &'a mut BitWriter,
        input: &'a [u8],
        input_base: usize,
        max_backward_distance: usize,
        symbol_limit: usize,
        max_static_commands: usize,
    ) -> Self {
        debug_assert!(u32::try_from(input.len()).is_ok());
        Self {
            output: writer,
            input,
            input_base,
            max_backward_distance,
            symbol_limit,
            max_static_commands,
            commands: Vec::with_capacity(symbol_limit.min(input.len() / 4 + 1)),
            literal_counts: [0; LITERAL_ALPHABET_SIZE],
            command_counts: [0; COMMAND_ALPHABET_SIZE],
            distance_counts: [0; DISTANCE_ALPHABET_SIZE],
            extra_bits: 0,
            symbols: 0,
            block_len: 0,
            has_copy: false,
            ring: [0; 4],
            ring_len: 0,
            result: Ok(()),
        }
    }

    /// Writes what is left, and advances `ring` past the copies. Input
    /// without copies goes out as one literal-only meta-block. Returns
    /// whether there were copies.
    pub(super) fn finish(mut self, ring: &mut DistanceRing) -> Result<bool, CompressError> {
        if !self.has_copy {
            // Only the last token can lack a copy, so nothing went out yet.
            write_compressed_literal_meta_block(self.output, self.input)?;
            return Ok(false);
        }
        self.flush();
        self.result?;
        let newest = core::array::from_fn::<_, 4, _>(|age| {
            self.ring[self.ring_len.wrapping_sub(age + 1) % 4]
        });
        ring.push_newest_first(&newest[..self.ring_len.min(4)]);
        Ok(true)
    }

    /// Writes the commands so far as one meta-block.
    #[inline(never)]
    fn flush(&mut self) {
        if self.result.is_ok() {
            self.result = self.write_meta_block();
        }
        self.commands.clear();
        self.literal_counts.fill(0);
        self.command_counts.fill(0);
        self.distance_counts.fill(0);
        self.extra_bits = 0;
        self.symbols = 0;
        self.block_len = 0;
    }

    fn write_meta_block(&mut self) -> Result<(), CompressError> {
        let writer = &mut *self.output;
        if self.block_len == 0 || self.block_len > MAX_META_BLOCK_SIZE {
            return Err(BurliError::Format("invalid compressed Brotli block size"));
        }
        write_meta_block_len(writer, self.block_len)?;
        write_block_and_context_header(writer)?;
        let (literals, commands, distances) = if self.commands.len() <= self.max_static_commands {
            let literals = write_static_literal_code(
                writer,
                self.input,
                self.block_len,
                &self.literal_counts,
            )?;
            write_static_command_and_distance_prefix_codes(writer);
            (
                literals,
                (
                    STATIC_COMMAND_CODES,
                    code_bits(&STATIC_COMMAND_CODES, &self.command_counts),
                ),
                (
                    STATIC_DISTANCE_CODES,
                    code_bits(&STATIC_DISTANCE_CODES, &self.distance_counts),
                ),
            )
        } else {
            let literals = write_code(writer, &self.literal_counts)?;
            let commands = write_code(writer, &self.command_counts)?;
            let mut distance_counts = self.distance_counts;
            if distance_counts.iter().all(|&count| count == 0) {
                distance_counts[0] = 1;
            }
            // A code of one symbol takes no bits, so the count for a
            // meta-block without distances adds none.
            let distances = write_code(writer, &distance_counts)?;
            (literals, commands, distances)
        };
        self.write_commands(literals, commands, distances);
        Ok(())
    }

    fn write_commands(
        &mut self,
        (literals, literal_bits): (Codes<LITERAL_ALPHABET_SIZE>, usize),
        (codes, command_bits): (Codes<COMMAND_ALPHABET_SIZE>, usize),
        (distances, distance_bits): (Codes<DISTANCE_ALPHABET_SIZE>, usize),
    ) {
        // Literal codes take at most 14 bits, so four fit one write.
        debug_assert!(
            literals
                .iter()
                .all(|&code| code as u8 <= FAST_CODE_BITS || code as u8 == u8::MAX)
        );
        let total_bits = literal_bits + command_bits + distance_bits + self.extra_bits;
        let (input, commands) = (self.input, &self.commands);
        self.output.write_bits_with(total_bits, |sink| {
            for command in commands {
                two_pass::write_command(sink, &codes, command.head);
                let start = command.insert_start as usize;
                let end = start + command.insert_len as usize;
                two_pass::write_literals(sink, &literals, &input[start..end]);
                if command.distance != NO_DISTANCE {
                    let code = distances[(command.distance & 0x3f) as usize];
                    let len = code as u8;
                    sink.write(
                        len + ((command.distance >> 6) & 0x1f) as u8,
                        u64::from(code >> 8) | ((command.distance >> 11) << len),
                    );
                }
            }
        });
    }
}

impl Sink for Writer<'_> {
    #[inline(always)]
    fn push(&mut self, token: Token) {
        if !self.commands.is_empty() && self.symbols >= self.symbol_limit {
            self.flush();
        }
        self.symbols = self
            .symbols
            .saturating_add(token.insert_len)
            .saturating_add(1);
        self.block_len = self.block_len.saturating_add(token.block_len());
        for &literal in &self.input[token.insert_start..token.insert_start + token.insert_len] {
            self.literal_counts[usize::from(literal)] += 1;
        }
        let (insert_code, insert_bits, insert_extra) = insert_code(token.insert_len);
        let (symbol, width, extra, distance) = if token.is_copy() {
            self.has_copy = true;
            debug_assert!(token.copy_len_code() >= 2);
            let (copy_code, copy_bits, copy_extra) = copy_code(token.copy_len_code());
            let width = insert_bits + copy_bits;
            let extra = insert_extra | (copy_extra << insert_bits);
            if token.use_last_distance && insert_code < 8 && copy_code < 16 {
                (
                    implicit_symbol(insert_code, copy_code),
                    width,
                    extra,
                    NO_DISTANCE,
                )
            } else {
                // Distance symbol 0 repeats the last distance.
                let (symbol, bits, distance_extra) = if token.use_last_distance {
                    (0, 0, 0)
                } else {
                    self.push_ring(token);
                    if let Some(code) = token.distance_code {
                        (usize::from(code), 0, 0)
                    } else if has_distance_code(token.distance) {
                        distance_code(token.distance)
                    } else {
                        self.result = Err(BurliError::Format("Brotli distance exceeds range"));
                        (0, 0, 0)
                    }
                };
                self.distance_counts[symbol] += 1;
                self.extra_bits += usize::from(bits);
                (
                    explicit_symbol(insert_code, copy_code),
                    width,
                    extra,
                    (distance_extra << 11) | (u64::from(bits) << 6) | symbol as u64,
                )
            }
        } else if insert_code < 8 {
            // The meta-block ends after the literals, so the copy length is
            // never used and no distance follows.
            (
                implicit_symbol(insert_code, 0),
                insert_bits,
                insert_extra,
                NO_DISTANCE,
            )
        } else {
            (
                explicit_symbol(insert_code, 0),
                insert_bits,
                insert_extra,
                NO_DISTANCE,
            )
        };
        self.command_counts[symbol] += 1;
        self.extra_bits += usize::from(width);
        self.commands.push(Command {
            head: symbol as u64 | (u64::from(width) << 10) | (extra << 16),
            distance,
            insert_start: token.insert_start as u32,
            insert_len: token.insert_len as u32,
        });
    }
}

impl Writer<'_> {
    /// Records the distance of a copy with a distance code, which the
    /// decoder pushes onto its ring unless it repeats the last one or
    /// reaches into the static dictionary.
    #[inline(always)]
    fn push_ring(&mut self, token: Token) {
        let pos = token.insert_start + token.insert_len;
        let max_distance = self
            .input_base
            .saturating_add(pos)
            .min(self.max_backward_distance);
        if token.distance_code != Some(0) && token.distance <= max_distance {
            self.ring[self.ring_len % 4] = token.distance;
            self.ring_len += 1;
        }
    }
}

/// Writes all `tokens` of `input` as in [`Writer`], for callers that keep
/// them and advance the distance ring themselves.
pub(super) fn write(
    writer: &mut BitWriter,
    input: &[u8],
    tokens: &[Token],
    symbol_limit: usize,
    max_static_commands: usize,
) -> Result<(), CompressError> {
    let mut sink = Writer::new(writer, input, 0, 0, symbol_limit, max_static_commands);
    for &token in tokens {
        sink.push(token);
    }
    sink.finish(&mut DistanceRing::default()).map(drop)
}

/// The bits `codes` take for `counts`.
fn code_bits<const N: usize>(codes: &Codes<N>, counts: &[u32; N]) -> usize {
    codes
        .iter()
        .zip(counts)
        .map(|(&code, &count)| count as usize * usize::from(code as u8))
        .sum()
}

/// Writes a prefix code for `counts` with [`prefix_code::write_tuned`].
/// Returns the codes and the bits they take for these counts.
fn write_code<const N: usize>(
    writer: &mut BitWriter,
    counts: &[u32; N],
) -> Result<(Codes<N>, usize), CompressError> {
    let max = counts.iter().copied().max().unwrap_or(0);
    if max < MAX_COUNT {
        return prefix_code::write_tuned(writer, counts);
    }
    // Only literals of long meta-blocks get here. Their code comes from
    // counts scaled down into the range the builder takes.
    let shift = max.ilog2() + 1 - MAX_COUNT.ilog2();
    let scaled = counts.map(|count| {
        if count == 0 {
            0
        } else {
            (count >> shift).max(1)
        }
    });
    let (codes, _) = prefix_code::write_tuned(writer, &scaled)?;
    Ok((codes, code_bits(&codes, counts)))
}

/// Writes the literal code of a meta-block with static command codes.
fn write_static_literal_code(
    writer: &mut BitWriter,
    input: &[u8],
    block_len: usize,
    counts: &[u32; LITERAL_ALPHABET_SIZE],
) -> Result<(Codes<LITERAL_ALPHABET_SIZE>, usize), CompressError> {
    if block_len > MAX_FAST_LITERAL_BLOCK {
        return write_code(writer, counts);
    }
    let frequencies = counts.map(|count| count as usize);
    let mut scratch = PrefixCodeScratch::default();
    scratch.reserve_for(LITERAL_ALPHABET_SIZE, LITERAL_ALPHABET_SIZE);
    let dense = if q2_tiny_balanced_literal_prefix_is_likely_safe(input) {
        write_balanced_fast_dense_prefix_code_array_from_frequencies_with_scratch(
            writer,
            &frequencies,
            &mut scratch,
        )?
    } else {
        write_fast_dense_prefix_code_array_from_frequencies_with_scratch(
            writer,
            &frequencies,
            &mut scratch,
        )?
    };
    Ok(packed_codes(&dense, counts))
}

/// `dense` as in [`Codes`], and the bits it takes for `counts`.
fn packed_codes<const N: usize>(
    dense: &[super::DenseSymbolCode; N],
    counts: &[u32; N],
) -> (Codes<N>, usize) {
    let mut bits = 0;
    let mut codes = [0; N];
    for ((code, dense), &count) in codes.iter_mut().zip(dense).zip(counts) {
        *code = (u32::from(dense.bits) << 8) | u32::from(dense.len);
        if count != 0 {
            bits += count as usize * usize::from(dense.len);
        }
    }
    (codes, bits)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encode::{q2, q4};

    /// A whole stream with the tokens of `input`.
    fn stream(input: &[u8], tokens: &[Token], symbol_limit: usize, max_static: usize) -> Vec<u8> {
        let mut writer = BitWriter::new();
        crate::metablock::write_window_bits(&mut writer, 22).unwrap();
        write(&mut writer, input, tokens, symbol_limit, max_static).unwrap();
        crate::metablock::write_last_empty_meta_block(&mut writer).unwrap();
        writer.into_bytes()
    }

    fn text() -> Vec<u8> {
        let mut input = Vec::new();
        for line in 0..2_000_u32 {
            let word = ["alpha", "beta", "gamma", "delta"][line as usize % 4];
            input
                .extend_from_slice(format!("{line:05} {word} {}\n", line * 7919 % 1000).as_bytes());
        }
        input
    }

    #[test]
    fn scales_large_counts() {
        let mut counts = [0; LITERAL_ALPHABET_SIZE];
        for (literal, count) in counts.iter_mut().enumerate().take(20) {
            *count = 1 << literal;
        }
        counts[200] = 1 << 25;
        let mut writer = BitWriter::new();
        let (codes, bits) = write_code(&mut writer, &counts).unwrap();
        assert_eq!(bits, code_bits(&codes, &counts));
        // Scaled counts keep every symbol, and the most frequent one gets
        // the shortest code.
        for (&code, &count) in codes.iter().zip(&counts) {
            assert_eq!(code as u8 == u8::MAX, count == 0);
        }
        assert_eq!(codes[200] as u8, 1);
    }

    #[test]
    fn writes_many_meta_blocks() {
        let input = text();
        let mut tokens = Vec::new();
        q2::collect_without_dictionary(&input, 1 << 20, &mut q2::Workspace::default(), &mut tokens);
        assert!(tokens.len() > 1_000);
        // Meta-blocks of a few hundred bytes, with static or own codes.
        for max_static in [0, super::super::Q2_MAX_STATIC_COMMANDS] {
            for symbol_limit in [64, 600, usize::MAX] {
                let encoded = stream(&input, &tokens, symbol_limit, max_static);
                assert_eq!(
                    burli_decode::decompress(&encoded).unwrap(),
                    input,
                    "limit {symbol_limit} static {max_static}"
                );
            }
        }
    }

    /// The ring after a [`Writer`] took `tokens`.
    fn streamed_ring(
        input: &[u8],
        tokens: &[Token],
        input_base: usize,
        max_backward_distance: usize,
    ) -> DistanceRing {
        let mut writer = BitWriter::new();
        let mut sink = Writer::new(
            &mut writer,
            input,
            input_base,
            max_backward_distance,
            600,
            0,
        );
        for &token in tokens {
            sink.push(token);
        }
        let mut ring = DistanceRing::default();
        assert!(sink.finish(&mut ring).unwrap());
        ring
    }

    #[test]
    fn advances_ring_like_decoder() {
        let input = text();
        for (input_base, max_backward_distance) in [(0, 1 << 20), (5_000, 1 << 20), (0, 100)] {
            let mut tokens = Vec::new();
            q4::collect(
                &input,
                input_base,
                max_backward_distance,
                &mut q4::Workspace::default(),
                &mut tokens,
            );
            let mut expected = DistanceRing::default();
            expected.advance_tokens(&tokens, input_base, max_backward_distance);
            let ring = streamed_ring(&input, &tokens, input_base, max_backward_distance);
            assert_eq!(
                ring, expected,
                "base {input_base} max {max_backward_distance}"
            );
            assert_ne!(ring, DistanceRing::default());
        }

        let copy = |insert_start, distance, distance_code, use_last_distance| Token {
            insert_start,
            insert_len: 20,
            copy_len: 8,
            copy_len_code: 0,
            distance,
            distance_code,
            use_last_distance,
        };
        let tokens = [
            copy(0, 9, None, false),
            copy(28, 5, None, false),
            // Repeats of the last distance and dictionary references leave
            // the ring as is.
            copy(56, 5, Some(0), false),
            copy(84, 5, Some(0), true),
            copy(112, 50_000, None, false),
            // Codes from the ring push their distance again.
            copy(140, 9, Some(1), false),
        ];
        let first = DistanceRing::default().last();
        assert_eq!(
            streamed_ring(&input, &tokens, 0, 1_000),
            DistanceRing([9, 5, 9, first])
        );
    }
}
