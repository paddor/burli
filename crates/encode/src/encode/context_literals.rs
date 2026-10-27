//! Literal-only meta-blocks with a literal context map, for blocks where
//! matching finds little.
//!
//! Binary data such as images and tables of numbers often depends on the
//! previous byte: the high byte of a 16-bit sample says much about the low
//! byte after it. Brotli picks one of several literal codes for each literal
//! by a context of the previous bytes. This writer takes the top six bits of
//! the previous byte as the context, counts the literals of each of the 64
//! contexts, and greedily merges the counts into a few codes, always the two
//! whose merge costs the fewest bits.

use alloc::{boxed::Box, vec, vec::Vec};

use burli_core::{CompressError, bits::BitWriter};

use super::{
    COMMAND_ALPHABET_SIZE, FAST_CODE_BITS, LITERAL_ALPHABET_SIZE, command_symbol_for_insert,
    insert_length_code, literal_runs, prefix_code, symbol_code, symbol_code_map,
    write_meta_block_len, write_prefix_code_from_frequencies, write_simple_prefix_code_single,
    write_var_len_u8,
};

const CONTEXTS: usize = 64;

/// [`write`] counts the first bytes of each this many of a sampled input.
pub(super) const SAMPLE_PERIOD: usize = 1024;

/// Brotli's literal context mode that takes the top six bits of the
/// previous byte.
const CONTEXT_MODE_MSB6: u64 = 1;

/// Estimated header bits of a literal code: these per used symbol and these
/// per code. They keep merges going when a separate code costs more than it
/// saves.
const HEADER_BITS_PER_SYMBOL: u64 = 5;
const HEADER_BITS_PER_CODE: u64 = 20;

/// Most used symbols for which [`merged_cost`] visits just those symbols,
/// not all.
const SPARSE_MAX_SYMBOLS: u32 = 160;

/// Fractional bits of the costs.
pub(super) const COST_SHIFT: u32 = 16;

/// Writes `input` as one meta-block of literals coded by the context of
/// the byte before each, with at most `max_codes` literal codes. `previous`
/// is the byte before `input` in the stream, or zero at its start. See
/// [`merge`] for `all_pairs_codes`. The counts take the first `sample`
/// bytes of every [`SAMPLE_PERIOD`], or all of them from `SAMPLE_PERIOD` up.
/// Returns `false` when the literals would take more bytes than `input`, so
/// the caller can store it instead.
pub(super) fn write(
    writer: &mut BitWriter,
    input: &[u8],
    previous: u8,
    max_codes: usize,
    all_pairs_codes: usize,
    sample: usize,
) -> Result<bool, CompressError> {
    debug_assert!((1..=CONTEXTS).contains(&max_codes));
    let mut counts = vec![[0_u32; LITERAL_ALPHABET_SIZE]; CONTEXTS];
    let exact = sample >= SAMPLE_PERIOD;
    if exact {
        count(&mut counts, input, previous);
    } else {
        for (index, period) in input.chunks(SAMPLE_PERIOD).enumerate() {
            let before = match index {
                0 => previous,
                _ => input[index * SAMPLE_PERIOD - 1],
            };
            count(&mut counts, &period[..period.len().min(sample)], before);
        }
    }
    let (codes, context_map) = merge(&mut counts, max_codes, all_pairs_codes);
    if !exact {
        // The sample may miss literals, so each counts once more.
        for &context in &codes {
            for count in &mut counts[context] {
                *count += 1;
            }
        }
    }

    let insert = insert_length_code(input.len())?;
    let header_start = writer.written_bits();
    write_meta_block_len(writer, input.len())?;
    // One block type each for literals, commands, and distances, and no
    // postfix or direct distance codes.
    write_var_len_u8(writer, 0)?;
    write_var_len_u8(writer, 0)?;
    write_var_len_u8(writer, 0)?;
    writer.write_bits(6, 0)?;
    writer.write_bits(2, CONTEXT_MODE_MSB6)?;
    write_context_map(writer, codes.len(), &context_map)?;
    // One distance code.
    write_var_len_u8(writer, 0)?;

    // Code bits and lengths by `code << 8 | literal`, apart, so a literal
    // takes two loads and no unpacking. A mask keeps indexes in bounds
    // without checks.
    let mut code_bits: Box<[u16; CONTEXTS << 8]> = vec![0; CONTEXTS << 8]
        .try_into()
        .expect("one row per context");
    let mut code_lens: Box<[u8; CONTEXTS << 8]> = vec![0; CONTEXTS << 8]
        .try_into()
        .expect("one row per context");
    let mut literal_bits = 0;
    for (index, &context) in codes.iter().enumerate() {
        let (literal_codes, bits) = prefix_code::write_tuned(writer, &counts[context])?;
        for (literal, &code) in literal_codes.iter().enumerate() {
            code_bits[index << 8 | literal] = (code >> 8) as u16;
            code_lens[index << 8 | literal] = code as u8;
        }
        literal_bits += bits;
    }
    write_simple_prefix_code_single(
        writer,
        COMMAND_ALPHABET_SIZE,
        command_symbol_for_insert(insert.code)?,
    )?;
    write_simple_prefix_code_single(writer, 64, 0)?;
    writer.write_bits_trusted(insert.extra_bits, u64::from(insert.extra));

    // The offset of each previous byte's code in the tables.
    let by_previous: [u16; 256] =
        core::array::from_fn(|byte| u16::from(context_map[byte >> 2]) << 8);
    // A stored meta-block takes at most 4 header bytes, and the literals
    // may take what is left of that. Exact codes tell their bits up front.
    // Sampled ones take an unknown number, and get room for that many. The
    // room past the literals spares the last run its check.
    let header_bits = writer.written_bits() - header_start;
    let Some(max_bits) = ((input.len() + 4) * 8).checked_sub(header_bits) else {
        return Ok(false);
    };
    if exact && literal_bits > max_bits {
        return Ok(false);
    }
    let fits = writer.write_bits_raw(max_bits + literal_runs::SLACK_BITS, |out, start| {
        let mut last = previous;
        let (end, fits) =
            literal_runs::write::<{ FAST_CODE_BITS as usize }, 4, 32>(out, start, input, |byte| {
                let index = (usize::from(by_previous[usize::from(last)]) | usize::from(byte))
                    & ((CONTEXTS << 8) - 1);
                last = byte;
                (u64::from(code_bits[index]), u32::from(code_lens[index]))
            });
        debug_assert!(!exact || (fits && end == start + literal_bits));
        (end, fits && end - start <= max_bits)
    });
    Ok(fits)
}

/// Counts each byte of `input` by the context of the byte before it, which
/// is `previous` for the first.
fn count(counts: &mut [[u32; LITERAL_ALPHABET_SIZE]], input: &[u8], previous: u8) {
    let mut last = previous;
    let (quads, rest) = input.as_chunks::<4>();
    for quad in quads {
        for &byte in quad {
            counts[usize::from(last >> 2)][usize::from(byte)] += 1;
            last = byte;
        }
    }
    for &byte in rest {
        counts[usize::from(last >> 2)][usize::from(byte)] += 1;
        last = byte;
    }
}

/// Merges the counts of contexts until at most `max_codes` remain and no
/// merge saves bits. Merges look at codes of neighboring contexts, which
/// follow bytes of about the same value, until `all_pairs_codes` remain, and
/// then at all pairs of codes. Returns the contexts whose counts now hold
/// each code's counts, and the code of each context.
fn merge(
    counts: &mut [[u32; LITERAL_ALPHABET_SIZE]],
    max_codes: usize,
    all_pairs_codes: usize,
) -> (Vec<usize>, [u8; CONTEXTS]) {
    let mut merger = Merger::new(counts);
    merger.run(max_codes.max(all_pairs_codes), false);
    if all_pairs_codes != 0 {
        merger.run(max_codes, true);
    }
    merger.finish()
}

/// Codes of contexts as they merge.
struct Merger<'a> {
    counts: &'a mut [[u32; LITERAL_ALPHABET_SIZE]],
    /// The contexts that hold the counts of each code, in order.
    codes: Vec<usize>,
    sizes: [Size; CONTEXTS],
    /// Bits a merge of two codes adds, by the lower and higher context.
    deltas: Vec<[i64; CONTEXTS]>,
    /// The context that holds the counts of each context's code.
    owners: [u8; CONTEXTS],
}

impl<'a> Merger<'a> {
    fn new(counts: &'a mut [[u32; LITERAL_ALPHABET_SIZE]]) -> Self {
        let codes = (0..CONTEXTS)
            .filter(|&context| counts[context].iter().any(|&count| count != 0))
            .collect::<Vec<_>>();
        let mut sizes = [Size::default(); CONTEXTS];
        for &context in &codes {
            let size = &mut sizes[context];
            for (symbol, &count) in counts[context].iter().enumerate() {
                size.total += count;
                size.used[symbol / 64] |= u64::from(count != 0) << (symbol % 64);
            }
            size.cost = merged_cost(
                &counts[context],
                &[0; LITERAL_ALPHABET_SIZE],
                size,
                &Size::default(),
            );
        }
        Self {
            counts,
            codes,
            sizes,
            deltas: vec![[0; CONTEXTS]; CONTEXTS],
            owners: core::array::from_fn(|context| context as u8),
        }
    }

    /// Merges the two codes whose merge adds the fewest bits until at most
    /// `max_codes` remain and no merge saves bits. Looks at all pairs with
    /// `all_pairs`, else at neighbors.
    fn run(&mut self, max_codes: usize, all_pairs: bool) {
        // The positions in `codes` after `index` that it may merge with.
        let partners = |codes: &[usize], index: usize| {
            let end = if all_pairs {
                codes.len()
            } else {
                (index + 2).min(codes.len())
            };
            (index + 1)..end
        };
        for index in 0..self.codes.len() {
            for other in partners(&self.codes, index) {
                self.update(self.codes[index], self.codes[other]);
            }
        }
        while self.codes.len() > 1 {
            let mut best = (i64::MAX, 0, 0);
            for index in 0..self.codes.len() {
                for other in partners(&self.codes, index) {
                    let (a, b) = (self.codes[index], self.codes[other]);
                    if self.deltas[a][b] < best.0 {
                        best = (self.deltas[a][b], a, b);
                    }
                }
            }
            let (bits, a, b) = best;
            if bits >= 0 && self.codes.len() <= max_codes {
                break;
            }
            self.merge(a, b, bits);
            // Only pairs with `a` changed.
            let position = self
                .codes
                .iter()
                .position(|&context| context == a)
                .unwrap_or(0);
            for index in 0..self.codes.len() {
                let pairs = partners(&self.codes, index);
                if index == position {
                    for other in pairs {
                        self.update(a, self.codes[other]);
                    }
                } else if pairs.contains(&position) {
                    self.update(self.codes[index], a);
                }
            }
        }
    }

    /// Updates the bits a merge of the codes of contexts `a` and `b` adds,
    /// for `a` below `b`.
    fn update(&mut self, a: usize, b: usize) {
        let (sizes, counts) = (&self.sizes, &self.counts);
        self.deltas[a][b] = merged_cost(&counts[a], &counts[b], &sizes[a], &sizes[b]) as i64
            - sizes[a].cost as i64
            - sizes[b].cost as i64;
    }

    /// Merges the code of context `b` into that of `a`, which adds `bits`.
    fn merge(&mut self, a: usize, b: usize, bits: i64) {
        let from = self.counts[b];
        for (count, &more) in self.counts[a].iter_mut().zip(&from) {
            *count += more;
        }
        let merged = self.sizes[b];
        let size = &mut self.sizes[a];
        size.total += merged.total;
        for (used, more) in size.used.iter_mut().zip(merged.used) {
            *used |= more;
        }
        size.cost = (size.cost as i64 + merged.cost as i64 + bits) as u64;
        self.codes.retain(|&context| context != b);
        for owner in &mut self.owners {
            if usize::from(*owner) == b {
                *owner = a as u8;
            }
        }
    }

    /// The contexts that hold each code's counts, and the code of each
    /// context.
    fn finish(mut self) -> (Vec<usize>, [u8; CONTEXTS]) {
        // Empty contexts take the first code.
        let mut context_map = [0_u8; CONTEXTS];
        for (context, &owner) in self.owners.iter().enumerate() {
            if let Some(code) = self
                .codes
                .iter()
                .position(|&kept| kept == usize::from(owner))
            {
                context_map[context] = code as u8;
            }
        }
        if self.codes.is_empty() {
            // Empty input still needs one code.
            self.codes.push(0);
        }
        (self.codes, context_map)
    }
}

/// Totals of the counts of a code, and its estimated cost.
#[derive(Clone, Copy, Debug, Default)]
struct Size {
    total: u32,
    /// Bit `symbol` is set when the count of `symbol` is not zero.
    used: [u64; 4],
    /// [`merged_cost`] of the counts alone.
    cost: u64,
}

/// Writes the number of literal codes and, for more than one, which code
/// each context takes, without run-length codes or the move-to-front
/// transform.
fn write_context_map(
    writer: &mut BitWriter,
    codes: usize,
    context_map: &[u8; CONTEXTS],
) -> Result<(), CompressError> {
    write_var_len_u8(writer, codes - 1)?;
    if codes == 1 {
        return Ok(());
    }
    // No run-length codes.
    writer.write_bits(1, 0)?;
    let mut uses = vec![0_usize; codes];
    for &code in context_map {
        uses[usize::from(code)] += 1;
    }
    let symbols = write_prefix_code_from_frequencies(writer, codes, &uses)?;
    let symbols = symbol_code_map(&symbols, codes);
    for &code in context_map {
        let symbol = symbol_code(&symbols, u16::from(code))?;
        writer.write_bits_trusted(symbol.len, u64::from(symbol.bits));
    }
    // No move-to-front transform.
    writer.write_bits(1, 0)
}

/// Estimated bits of the literals counted by `a` plus `b`, with sizes
/// `a_size` and `b_size`, under their own code, and of that code's header,
/// scaled by `1 << COST_SHIFT`.
fn merged_cost(
    a: &[u32; LITERAL_ALPHABET_SIZE],
    b: &[u32; LITERAL_ALPHABET_SIZE],
    a_size: &Size,
    b_size: &Size,
) -> u64 {
    let used: [u64; 4] = core::array::from_fn(|word| a_size.used[word] | b_size.used[word]);
    let used_count: u32 = used.iter().map(|word| word.count_ones()).sum();
    let mut sum = 0_u64;
    if used_count > SPARSE_MAX_SYMBOLS {
        for (&a, &b) in a.iter().zip(b) {
            sum += n_log2_n(a + b);
        }
    } else {
        // Short blocks leave many symbols of a context unused.
        for (word, &bits) in used.iter().enumerate() {
            let mut bits = bits;
            while bits != 0 {
                let symbol = (word * 64 + bits.trailing_zeros() as usize) % LITERAL_ALPHABET_SIZE;
                sum += n_log2_n(a[symbol] + b[symbol]);
                bits &= bits - 1;
            }
        }
    }
    n_log2_n(a_size.total + b_size.total) - sum
        + ((u64::from(used_count) * HEADER_BITS_PER_SYMBOL + HEADER_BITS_PER_CODE) << COST_SHIFT)
}

/// `n * log2(n)`, scaled by `1 << COST_SHIFT`, with ten bits of the
/// mantissa.
#[inline(always)]
pub(super) fn n_log2_n(n: u32) -> u64 {
    match SMALL_N_LOG2_N.get(n as usize) {
        Some(&value) => u64::from(value),
        None => n_log2_n_from(n, &LOG2_FRACTIONS),
    }
}

/// [`n_log2_n`] with a table of `log2(1 + i / 1024)` for each `i`.
const fn n_log2_n_from(n: u32, fractions: &[u32; 1024]) -> u64 {
    if n == 0 {
        return 0;
    }
    let exponent = 31 - n.leading_zeros();
    let mantissa = ((n << n.leading_zeros()) >> 21) & 1023;
    n as u64 * (((exponent as u64) << COST_SHIFT) + fractions[mantissa as usize] as u64)
}

/// [`n_log2_n`] of counts below 4096, which are most counts of a meta-block
/// of a MiB.
static SMALL_N_LOG2_N: [u32; 4096] = {
    let fractions = log2_fractions();
    let mut table = [0; 4096];
    let mut n = 0;
    while n < 4096 {
        table[n] = n_log2_n_from(n as u32, &fractions) as u32;
        n += 1;
    }
    table
};

static LOG2_FRACTIONS: [u32; 1024] = log2_fractions();

/// `log2(1 + i / 1024)` for each `i`, scaled by `1 << COST_SHIFT`.
const fn log2_fractions() -> [u32; 1024] {
    let mut table = [0; 1024];
    let mut index = 0;
    while index < 1024 {
        // `1 + index / 1024` with 30 fractional bits. Squaring doubles the
        // logarithm, so each square past 2 gives the next bit.
        let mut x = (1024 + index as u64) << 20;
        let mut fraction = 0;
        let mut bit = 0;
        while bit < COST_SHIFT {
            x = (x * x) >> 30;
            fraction <<= 1;
            if x >= 2 << 30 {
                x >>= 1;
                fraction |= 1;
            }
            bit += 1;
        }
        table[index] = fraction;
        index += 1;
    }
    table
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log2_fractions_are_close() {
        assert_eq!(LOG2_FRACTIONS[0], 0);
        // log2(1.5) = 0.5849625
        assert_eq!(LOG2_FRACTIONS[512], 38_336);
        assert_eq!(n_log2_n(1), 0);
        assert_eq!(n_log2_n(2), 2 << COST_SHIFT);
        assert_eq!(n_log2_n(1024), (1024 * 10) << COST_SHIFT);
        assert_eq!(n_log2_n(4095), n_log2_n_from(4095, &LOG2_FRACTIONS));
        assert_eq!(n_log2_n(1 << 20), (20 << 20) << COST_SHIFT);
    }

    #[test]
    fn merges_alike_contexts() {
        let mut counts = vec![[0_u32; LITERAL_ALPHABET_SIZE]; CONTEXTS];
        // Contexts 0 and 1 hold low bytes, 2 and 3 high bytes.
        for byte in 0..16 {
            counts[0][byte] = 1000;
            counts[1][byte] = 900;
            counts[2][byte + 240] = 1000;
            counts[3][byte + 240] = 1100;
        }
        let (codes, context_map) = merge(&mut counts.clone(), 2, CONTEXTS);
        assert_eq!(codes.len(), 2);
        assert_eq!(context_map[0], context_map[1]);
        assert_eq!(context_map[2], context_map[3]);
        assert_ne!(context_map[0], context_map[2]);
        let (codes, context_map) = merge(&mut counts, 2, 0);
        assert_eq!(codes.len(), 2);
        assert_eq!(context_map[0], context_map[1]);
        assert_eq!(context_map[2], context_map[3]);
        assert_ne!(context_map[0], context_map[2]);
    }
}
