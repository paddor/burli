//! Prefix codes for the one-pass q0 and two-pass q1 encoders.
//!
//! The work stays on the symbols that occur, which are few in short inputs:
//! an in-place Huffman pass over their sorted counts, canonical codes in one
//! pass over them, and code lengths written run by run, with a table for the
//! zero runs between them. Lengths go out with the static code-length code,
//! as in [`super::write_fast_complex_prefix_code_lengths`], which writes the
//! same bits, or with a code-length code built for them. That code takes a
//! pass to count the runs and a Huffman code of 18 symbols, and saves up to
//! a quarter of the length bits.

use burli_core::{
    CompressError,
    bits::{BitWriter, MAX_BITS_PER_OP},
};

use super::{
    CODE_LENGTH_ALPHABET_SIZE, CODE_LENGTH_ORDER, COMMAND_ALPHABET_SIZE, FAST_CODE_BITS,
    LITERAL_ALPHABET_SIZE, MAX_SIMPLE_PREFIX_SYMBOLS, MISSING_DENSE_SYMBOL_CODE,
    STATIC_CODE_LENGTH_BITS, STATIC_CODE_LENGTH_DEPTH, write_simple_dense_prefix_code,
};

/// The largest alphabet, of commands.
const MAX_SYMBOLS: usize = COMMAND_ALPHABET_SIZE;

/// Codes by symbol, each `bits << 8 | len` with the bits reversed for
/// LSB-first output. One `u32` per code fills and loads faster than a
/// struct with a padding byte.
pub(super) type Codes<const N: usize> = [u32; N];

/// Literal codes by literal.
pub(super) type LiteralCodes = Codes<LITERAL_ALPHABET_SIZE>;

/// Code of symbols that do not occur, with length [`u8::MAX`].
const MISSING: u32 = u8::MAX as u32;

/// Header that selects the static code-length code.
const STATIC_CODE_LENGTH_HEADER: (u8, u64) = (40, 0x00ff_5555_5554);

/// Most bits of a code-length code header: the skip count and 18 code
/// lengths.
const MAX_HEADER_BITS: usize = 2 + 4 * CODE_LENGTH_ALPHABET_SIZE;

/// Most bits one used symbol adds to the code lengths: a zero run before it
/// and its own length code. Runs of equal lengths cost less per symbol.
const MAX_BITS_PER_SYMBOL: usize = 32 + 5;

/// Longest code of a code-length code.
const MAX_CODE_LENGTH_BITS: u32 = 5;

/// Extra bits after each code-length symbol: repeat codes 16 and 17 carry
/// counts.
const CODE_LENGTH_EXTRA_BITS: [u8; CODE_LENGTH_ALPHABET_SIZE] =
    [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 3];

/// Width and bits that code each code length of a code-length code.
static CODE_LENGTH_LENGTH_CODES: [(u8, u8); 6] = [(2, 0), (4, 7), (3, 3), (2, 2), (2, 1), (4, 15)];

/// Writes a prefix code for `counts`, indexed by symbol, with the static
/// code-length code. Returns the codes and the bits they take for these
/// counts. Codes are at most [`FAST_CODE_BITS`] long. Counts must be below
/// 2^22.
pub(super) fn write<const N: usize>(
    writer: &mut BitWriter,
    counts: &[u32; N],
) -> Result<(Codes<N>, usize), CompressError> {
    write_with::<N, false, FAST_CODE_BITS>(writer, counts)
}

/// [`write`] with codes of at most `MAX_BITS`, up to [`FAST_CODE_BITS`].
pub(super) fn write_with_max_bits<const N: usize, const MAX_BITS: u8>(
    writer: &mut BitWriter,
    counts: &[u32; N],
) -> Result<(Codes<N>, usize), CompressError> {
    write_with::<N, false, MAX_BITS>(writer, counts)
}

/// [`write`], with a code-length code built for the lengths when that takes
/// fewer bits than the static one.
pub(super) fn write_tuned<const N: usize>(
    writer: &mut BitWriter,
    counts: &[u32; N],
) -> Result<(Codes<N>, usize), CompressError> {
    write_with::<N, true, FAST_CODE_BITS>(writer, counts)
}

#[inline(always)]
fn write_with<const N: usize, const TUNED: bool, const MAX_BITS: u8>(
    writer: &mut BitWriter,
    counts: &[u32; N],
) -> Result<(Codes<N>, usize), CompressError> {
    const { assert!(N.is_multiple_of(8) && N <= MAX_SYMBOLS && MAX_BITS <= FAST_CODE_BITS) };
    // Sort keys are `count << shift | symbol`.
    let shift = N.next_power_of_two().trailing_zeros();
    let symbol_mask = (1 << shift) - 1;

    // Used symbols in symbol order, and their sort keys.
    let mut order = [0_u16; N];
    let mut keys = [0_u32; N];
    let mut used = 0;
    // Short inputs use few symbols, so most groups of 8 are all zero.
    for (group, group_counts) in counts.as_chunks::<8>().0.iter().enumerate() {
        if group_counts.iter().fold(0, |any, &count| any | count) == 0 {
            continue;
        }
        for (offset, &count) in group_counts.iter().enumerate() {
            debug_assert!(count < 1 << 22);
            let symbol = group * 8 + offset;
            // `used` is at most `symbol` here.
            let slot = if N.is_power_of_two() {
                used & (N - 1)
            } else {
                used.min(N - 1)
            };
            order[slot] = symbol as u16;
            keys[slot] = (count << shift) | symbol as u32;
            used += usize::from(count != 0);
        }
    }

    let mut codes = [MISSING; N];
    if used <= MAX_SIMPLE_PREFIX_SYMBOLS {
        let symbols = &order[..used.max(1)];
        let mut simple = [MISSING_DENSE_SYMBOL_CODE; N];
        write_simple_dense_prefix_code(writer, symbols, &mut simple)?;
        let mut bits = 0;
        for &symbol in symbols {
            let code = simple[usize::from(symbol)];
            codes[usize::from(symbol)] = (u32::from(code.bits) << 8) | u32::from(code.len);
            bits += counts[usize::from(symbol)] as usize * usize::from(code.len);
        }
        return Ok((codes, bits));
    }

    let keys = &mut keys[..used];
    keys.sort_unstable();
    let mut lengths = [0_u8; N];
    let length_counts = huffman_lengths::<N, MAX_BITS>(keys, symbol_mask, &mut lengths);

    let mut next_code = [0_u16; FAST_CODE_BITS as usize + 1];
    let mut code = 0_u16;
    for bits in 1..next_code.len() {
        code = (code + length_counts[bits - 1]) << 1;
        next_code[bits] = code;
    }
    let mut bits = 0;
    for &symbol in &order[..used] {
        let symbol = usize::from(symbol);
        let len = lengths[symbol];
        let slot = &mut next_code[usize::from(len)];
        let code = *slot;
        *slot = code + 1;
        let reversed = code.reverse_bits() >> (16 - len);
        codes[symbol] = (u32::from(reversed) << 8) | u32::from(len);
        bits += counts[symbol] as usize * usize::from(len);
    }

    if TUNED {
        write_tuned_lengths(writer, &order[..used], &lengths);
    } else {
        write_lengths(writer, &order[..used], &lengths);
    }
    Ok((codes, bits))
}

/// Writes Huffman code lengths of at most `MAX_BITS` into
/// `lengths[symbol]` for `keys`, sorted `count << shift | symbol` for at
/// least two symbols, and returns the number of codes of each length.
/// Counts below a floor are raised until the code fits, as Google's encoder
/// does.
fn huffman_lengths<const N: usize, const MAX_BITS: u8>(
    keys: &[u32],
    symbol_mask: u32,
    lengths: &mut [u8; N],
) -> [u16; 16] {
    let shift = symbol_mask.count_ones();
    let mut nodes = [0_u32; N];
    let nodes = &mut nodes[..keys.len()];
    let mut floor = 0;
    let length_counts = loop {
        // Raising a floor keeps the counts sorted.
        for (node, &key) in nodes.iter_mut().zip(keys) {
            *node = (key >> shift).max(floor);
        }
        let length_counts = in_place_lengths(nodes);
        if nodes[0] <= u32::from(MAX_BITS) {
            break length_counts;
        }
        floor = (floor * 2).max(2);
    };
    for (&key, &len) in keys.iter().zip(nodes.iter()) {
        lengths[(key & symbol_mask) as usize] = len as u8;
    }
    length_counts
}

/// Replaces the ascending counts in `nodes` with Huffman code lengths, by
/// Moffat and Katajainen's in-place method, and returns the number of codes
/// of each length. `nodes[0]` gets the longest code. Lengths past 15 are
/// left out of the counts, so callers check `nodes[0]` first.
fn in_place_lengths(nodes: &mut [u32]) -> [u16; 16] {
    let n = nodes.len();
    debug_assert!(n >= 2);
    debug_assert!(nodes.is_sorted());

    // Left to right, node `next` becomes the parent of the two smallest
    // roots or leaves. A merged node's slot then holds its parent's index.
    nodes[0] += nodes[1];
    let mut root = 0;
    let mut leaf = 2;
    for next in 1..n - 1 {
        if leaf >= n || nodes[root] < nodes[leaf] {
            nodes[next] = nodes[root];
            nodes[root] = next as u32;
            root += 1;
        } else {
            nodes[next] = nodes[leaf];
            leaf += 1;
        }
        if leaf >= n || (root < next && nodes[root] < nodes[leaf]) {
            nodes[next] += nodes[root];
            nodes[root] = next as u32;
            root += 1;
        } else {
            nodes[next] += nodes[leaf];
            leaf += 1;
        }
    }

    // Right to left, each internal node's depth from its parent's. Node
    // `n - 2` is the root.
    nodes[n - 2] = 0;
    for next in (0..n - 2).rev() {
        nodes[next] = nodes[nodes[next] as usize] + 1;
    }

    // Right to left, leaf depths: each level has room for twice the
    // internal nodes of the level above, and leaves fill what internal
    // nodes leave free.
    let mut length_counts = [0_u16; 16];
    let mut free = 1;
    let mut depth = 0;
    let mut internal = n - 1;
    let mut next = n;
    while free > 0 {
        let mut level = 0;
        while internal > 0 && nodes[internal - 1] == depth {
            level += 1;
            internal -= 1;
        }
        length_counts[(depth & 15) as usize] = (free - level) as u16;
        while free > level {
            next -= 1;
            nodes[next] = depth;
            free -= 1;
        }
        free = 2 * level;
        depth += 1;
    }
    length_counts
}

/// Writes the code lengths of `used`, in symbol order, with the static
/// code-length code. Runs repeat by the rules of
/// [`super::encode_code_length_tree_into`].
fn write_lengths<const N: usize>(writer: &mut BitWriter, used: &[u16], lengths: &[u8; N]) {
    let max_bits = MAX_HEADER_BITS + MAX_BITS_PER_SYMBOL * used.len();
    writer.write_bits_up_to(max_bits, |mut sink| {
        sink.write(STATIC_CODE_LENGTH_HEADER.0, STATIC_CODE_LENGTH_HEADER.1);
        let mut previous = 8;
        let mut expected = 0;
        let mut start = 0;
        while start < used.len() {
            let symbol = usize::from(used[start]);
            let value = lengths[symbol];
            let mut end = start + 1;
            while end < used.len()
                && usize::from(used[end]) == symbol + (end - start)
                && lengths[usize::from(used[end])] == value
            {
                end += 1;
            }
            let zeros = ZERO_RUNS[symbol - expected];
            let zero_width = (zeros >> 32) as u8;
            let zero_bits = zeros & 0xffff_ffff;
            let (width, bits) = value_run(value, end - start, previous);
            if zero_width + width <= MAX_BITS_PER_OP {
                sink.write(zero_width + width, zero_bits | (bits << zero_width));
            } else {
                sink.write(zero_width, zero_bits);
                sink.write(width, bits);
            }
            previous = value;
            expected = symbol + (end - start);
            start = end;
        }
        (sink, ())
    });
}

/// Writes the code lengths of `used`, in symbol order, with a code-length
/// code built for their runs, or with the static one when that takes fewer
/// bits.
fn write_tuned_lengths<const N: usize>(writer: &mut BitWriter, used: &[u16], lengths: &[u8; N]) {
    // Code-length symbols, each with its extra bits from bit 5. Each codes
    // at least one length, so there are at most `N`.
    let mut entries = [0_u8; N];
    let mut len = 0;
    let mut counts = [0_u32; CODE_LENGTH_ALPHABET_SIZE];
    for_each_length_symbol(used, lengths, |symbol, extra| {
        entries[len] = symbol | (extra << 5);
        len += 1;
        counts[usize::from(symbol)] += 1;
    });
    let entries = &entries[..len];

    let tuned = tuned_code_length_code(&counts);
    let max_bits = MAX_HEADER_BITS + MAX_BITS_PER_SYMBOL * used.len();
    writer.write_bits_up_to(max_bits, |mut sink| {
        let codes = match &tuned {
            Some(tuned) => {
                for &(width, bits) in &tuned.header[..tuned.header_len] {
                    sink.write(width, u64::from(bits));
                }
                &tuned.codes
            }
            None => {
                sink.write(STATIC_CODE_LENGTH_HEADER.0, STATIC_CODE_LENGTH_HEADER.1);
                &STATIC_CODE_LENGTH_CODES
            }
        };
        // Entries take at most 8 bits, so 7 fit one write.
        for group in entries.chunks(7) {
            let mut bits = 0;
            let mut width = 0;
            for &entry in group {
                let code = codes[usize::from(entry & 31)];
                let len = (code >> 8) as u8;
                bits |= (u64::from(code >> 16) | (u64::from(entry >> 5) << len)) << width;
                width += code as u8;
            }
            sink.write(width, bits);
        }
        (sink, ())
    });
}

/// A code-length code built for some code lengths.
struct TunedCode {
    /// Width and bits of each header field: the skip count, then code
    /// lengths.
    header: [(u8, u8); CODE_LENGTH_ALPHABET_SIZE + 1],
    header_len: usize,
    /// Codes as in [`STATIC_CODE_LENGTH_CODES`].
    codes: [u32; CODE_LENGTH_ALPHABET_SIZE],
}

/// A code-length code for code-length symbols with these counts, or `None`
/// when the static code takes no more bits.
fn tuned_code_length_code(counts: &[u32; CODE_LENGTH_ALPHABET_SIZE]) -> Option<TunedCode> {
    // Sort keys are `count << 5 | symbol`.
    let mut keys = [0_u32; CODE_LENGTH_ALPHABET_SIZE];
    let mut kinds = 0;
    for (symbol, &count) in counts.iter().enumerate() {
        keys[kinds] = (count << 5) | symbol as u32;
        kinds += usize::from(count != 0);
    }
    if kinds < 2 {
        // One symbol would take no bits, a case the static code leaves out.
        return None;
    }
    let keys = &mut keys[..kinds];
    keys.sort_unstable();
    let mut nodes = [0_u32; CODE_LENGTH_ALPHABET_SIZE];
    let nodes = &mut nodes[..kinds];
    let mut floor = 0;
    let length_counts = loop {
        for (node, &key) in nodes.iter_mut().zip(&*keys) {
            *node = (key >> 5).max(floor);
        }
        let length_counts = in_place_lengths(nodes);
        if nodes[0] <= MAX_CODE_LENGTH_BITS {
            break length_counts;
        }
        floor = (floor * 2).max(2);
    };
    let mut code_lengths = [0_u8; CODE_LENGTH_ALPHABET_SIZE];
    for (&key, &len) in keys.iter().zip(nodes.iter()) {
        code_lengths[(key & 31) as usize] = len as u8;
    }

    // Lengths in the order the header lists them, without trailing zeros,
    // after a skip of 2 or 3 leading zeros.
    let stored = CODE_LENGTH_ORDER
        .iter()
        .rposition(|&symbol| code_lengths[usize::from(symbol)] != 0)
        .map_or(0, |index| index + 1);
    let skip = match [0, 1, 2].map(|index| code_lengths[usize::from(CODE_LENGTH_ORDER[index])]) {
        [0, 0, 0] => 3,
        [0, 0, _] => 2,
        _ => 0,
    };
    let mut tuned = TunedCode {
        header: [(0, 0); CODE_LENGTH_ALPHABET_SIZE + 1],
        header_len: 1,
        codes: [0; CODE_LENGTH_ALPHABET_SIZE],
    };
    tuned.header[0] = (2, skip as u8);
    let mut tuned_bits = 2;
    for &symbol in &CODE_LENGTH_ORDER[skip..stored] {
        let field = CODE_LENGTH_LENGTH_CODES[usize::from(code_lengths[usize::from(symbol)])];
        tuned.header[tuned.header_len] = field;
        tuned.header_len += 1;
        tuned_bits += usize::from(field.0);
    }
    let mut static_bits = usize::from(STATIC_CODE_LENGTH_HEADER.0);
    for (symbol, &count) in counts.iter().enumerate() {
        tuned_bits += count as usize * usize::from(code_lengths[symbol]);
        static_bits += count as usize * usize::from(STATIC_CODE_LENGTH_DEPTH[symbol]);
    }
    if static_bits <= tuned_bits {
        return None;
    }

    // Canonical codes, by length and then symbol.
    let mut next_code = [0_u8; MAX_CODE_LENGTH_BITS as usize + 1];
    let mut code = 0;
    for bits in 1..next_code.len() {
        code = (code + length_counts[bits - 1] as u8) << 1;
        next_code[bits] = code;
    }
    for (symbol, &len) in code_lengths.iter().enumerate() {
        if len != 0 {
            let code = next_code[usize::from(len)];
            next_code[usize::from(len)] = code + 1;
            let bits = u16::from(code.reverse_bits() >> (8 - len));
            tuned.codes[symbol] = code_length_code(symbol, len, bits);
        }
    }
    Some(tuned)
}

/// Code-length code entry for `symbol`: `bits << 16 | len << 8 | width`,
/// where `width` adds the extra bits of repeat codes.
const fn code_length_code(symbol: usize, len: u8, bits: u16) -> u32 {
    ((bits as u32) << 16) | ((len as u32) << 8) | (len + CODE_LENGTH_EXTRA_BITS[symbol]) as u32
}

/// The static code-length code, as in [`code_length_code`].
static STATIC_CODE_LENGTH_CODES: [u32; CODE_LENGTH_ALPHABET_SIZE] = {
    let mut codes = [0; CODE_LENGTH_ALPHABET_SIZE];
    let mut symbol = 0;
    while symbol < CODE_LENGTH_ALPHABET_SIZE {
        codes[symbol] = code_length_code(
            symbol,
            STATIC_CODE_LENGTH_DEPTH[symbol],
            STATIC_CODE_LENGTH_BITS[symbol],
        );
        symbol += 1;
    }
    codes
};

/// Calls `emit` with each code-length symbol and its extra bits that code
/// the lengths of `used`, in order, with the runs of [`write_lengths`].
#[inline(always)]
fn for_each_length_symbol<const N: usize>(
    used: &[u16],
    lengths: &[u8; N],
    mut emit: impl FnMut(u8, u8),
) {
    let mut previous = 8;
    let mut expected = 0;
    let mut start = 0;
    while start < used.len() {
        let symbol = usize::from(used[start]);
        let value = lengths[symbol];
        let mut end = start + 1;
        while end < used.len()
            && usize::from(used[end]) == symbol + (end - start)
            && lengths[usize::from(used[end])] == value
        {
            end += 1;
        }

        let mut zeros = symbol - expected;
        if zeros == 11 {
            emit(0, 0);
            zeros -= 1;
        }
        if zeros < 3 {
            for _ in 0..zeros {
                emit(0, 0);
            }
        } else {
            emit_repeats(17, 3, zeros, &mut emit);
        }

        let mut reps = end - start;
        if previous != value {
            emit(value, 0);
            reps -= 1;
        }
        if reps == 7 {
            emit(value, 0);
            reps -= 1;
        }
        if reps < 3 {
            for _ in 0..reps {
                emit(value, 0);
            }
        } else {
            emit_repeats(16, 2, reps, &mut emit);
        }

        previous = value;
        expected = symbol + (end - start);
        start = end;
    }
}

/// Emits repeat code `code` for `reps` of at least 3, with the count in
/// base `1 << digit_bits`, most significant digit first.
#[inline(always)]
fn emit_repeats(code: u8, digit_bits: u32, reps: usize, emit: &mut impl FnMut(u8, u8)) {
    let mut digits = [0_u8; 8];
    let mut count = 0;
    let mut reps = reps - 3;
    loop {
        digits[count] = (reps & ((1 << digit_bits) - 1)) as u8;
        count += 1;
        reps >>= digit_bits;
        if reps == 0 {
            break;
        }
        reps -= 1;
    }
    for &digit in digits[..count].iter().rev() {
        emit(code, digit);
    }
}

/// Width and bits of a run of `reps` code lengths `value`, after
/// `previous`, the last nonzero length written. Repeat code 16 carries the
/// count in base 4, most significant digit first.
#[inline(always)]
fn value_run(value: u8, mut reps: usize, previous: u8) -> (u8, u64) {
    let (value_width, value_bits) = static_code(value);
    if reps == 1 {
        return (value_width, value_bits);
    }
    let mut width = 0;
    let mut bits = 0;
    let mut emit = |code_width: u8, code_bits: u64| {
        bits |= code_bits << width;
        width += code_width;
    };
    if previous != value {
        emit(value_width, value_bits);
        reps -= 1;
    }
    if reps == 7 {
        emit(value_width, value_bits);
        reps -= 1;
    }
    if reps < 3 {
        for _ in 0..reps {
            emit(value_width, value_bits);
        }
        return (width, bits);
    }
    let mut digits = [0_u8; 8];
    let mut count = 0;
    reps -= 3;
    loop {
        digits[count] = (reps & 3) as u8;
        count += 1;
        reps >>= 2;
        if reps == 0 {
            break;
        }
        reps -= 1;
    }
    let (repeat_width, repeat_bits) = static_code(16);
    for &digit in digits[..count].iter().rev() {
        emit(
            repeat_width + 2,
            repeat_bits | (u64::from(digit) << repeat_width),
        );
    }
    (width, bits)
}

#[inline(always)]
fn static_code(symbol: u8) -> (u8, u64) {
    let symbol = usize::from(symbol);
    (
        STATIC_CODE_LENGTH_DEPTH[symbol],
        u64::from(STATIC_CODE_LENGTH_BITS[symbol]),
    )
}

/// Static code-length codes for runs of zero lengths by run length, each
/// `width << 32 | bits`. Repeat code 17 carries the count in base 8, most
/// significant digit first.
static ZERO_RUNS: [u64; MAX_SYMBOLS] = {
    let mut table = [0; MAX_SYMBOLS];
    let mut run = 1;
    while run < MAX_SYMBOLS {
        let (zero_width, zero_bits) = (
            STATIC_CODE_LENGTH_DEPTH[0] as u64,
            STATIC_CODE_LENGTH_BITS[0] as u64,
        );
        let (repeat_width, repeat_bits) = (
            STATIC_CODE_LENGTH_DEPTH[17] as u64,
            STATIC_CODE_LENGTH_BITS[17] as u64,
        );
        let mut width = 0;
        let mut bits = 0;
        let mut reps = run;
        if reps == 11 {
            bits |= zero_bits << width;
            width += zero_width;
            reps -= 1;
        }
        if reps < 3 {
            while reps > 0 {
                bits |= zero_bits << width;
                width += zero_width;
                reps -= 1;
            }
        } else {
            let mut digits = [0_u64; 8];
            let mut count = 0;
            reps -= 3;
            loop {
                digits[count] = (reps & 7) as u64;
                count += 1;
                reps >>= 3;
                if reps == 0 {
                    break;
                }
                reps -= 1;
            }
            while count > 0 {
                count -= 1;
                bits |= (repeat_bits | (digits[count] << repeat_width)) << width;
                width += repeat_width + 3;
            }
        }
        table[run] = (width << 32) | bits;
        run += 1;
    }
    table
};

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    /// Pseudo-random counts with `used` nonzero entries, some skewed.
    fn sample_counts<const N: usize>(seed: u64, used: usize) -> [u32; N] {
        let mut state = seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let mut counts = [0; N];
        let mut placed = 0;
        while placed < used {
            let symbol = (next() % N as u64) as usize;
            if counts[symbol] == 0 {
                let shift = next() % 16;
                counts[symbol] = 1 + (next() % (1 << shift)) as u32;
                placed += 1;
            }
        }
        counts
    }

    fn check_lengths<const N: usize>() {
        let shift = N.next_power_of_two().trailing_zeros();
        for seed in 0..400 {
            let used = 5 + (seed as usize * 7) % (N - 4);
            let counts = sample_counts::<N>(seed, used);
            let mut keys: Vec<u32> = (0..N)
                .filter(|&symbol| counts[symbol] != 0)
                .map(|symbol| (counts[symbol] << shift) | symbol as u32)
                .collect();
            keys.sort_unstable();
            let mut lengths = [0; N];
            let length_counts =
                huffman_lengths::<N, FAST_CODE_BITS>(&keys, (1 << shift) - 1, &mut lengths);

            let frequencies: Vec<usize> = counts.iter().map(|&count| count as usize).collect();
            // Codes that need no length limit are optimal, so their cost is
            // fixed. Limited codes depend on which counts the floor raises.
            let expected = super::super::huffman_code_lengths(&frequencies, FAST_CODE_BITS);
            let cost = |lengths: &[u8]| -> u64 {
                lengths
                    .iter()
                    .zip(&counts)
                    .map(|(&len, &count)| u64::from(len) * u64::from(count))
                    .sum()
            };
            if let Some(expected) = expected {
                assert_eq!(cost(&lengths), cost(&expected), "N {N} seed {seed}");
            }

            let kraft: u64 = lengths
                .iter()
                .filter(|&&len| len != 0)
                .map(|&len| 1 << (FAST_CODE_BITS - len))
                .sum();
            assert_eq!(kraft, 1 << FAST_CODE_BITS, "N {N} seed {seed}");
            for (len, &count) in length_counts.iter().enumerate().skip(1) {
                let actual = lengths.iter().filter(|&&l| usize::from(l) == len).count();
                assert_eq!(usize::from(count), actual, "N {N} seed {seed} len {len}");
            }
            for (symbol, &count) in counts.iter().enumerate() {
                assert_eq!(count == 0, lengths[symbol] == 0, "N {N} seed {seed}");
            }
        }
    }

    #[test]
    fn lengths_match_general_builder_cost() {
        check_lengths::<64>();
        check_lengths::<LITERAL_ALPHABET_SIZE>();
        check_lengths::<COMMAND_ALPHABET_SIZE>();
    }

    #[test]
    fn deep_codes_are_limited() {
        // Fibonacci counts would need codes of up to 24 bits.
        let mut counts = [0; LITERAL_ALPHABET_SIZE];
        let (mut a, mut b) = (1, 1);
        for count in counts.iter_mut().take(25) {
            *count = a;
            (a, b) = (b, a + b);
        }
        let mut keys: Vec<u32> = (0..25)
            .map(|symbol| (counts[symbol] << 8) | symbol as u32)
            .collect();
        keys.sort_unstable();
        let mut lengths = [0; LITERAL_ALPHABET_SIZE];
        huffman_lengths::<LITERAL_ALPHABET_SIZE, FAST_CODE_BITS>(&keys, 0xff, &mut lengths);
        assert!(
            lengths[..25]
                .iter()
                .all(|&len| (1..=FAST_CODE_BITS).contains(&len))
        );
        let kraft: u64 = lengths[..25]
            .iter()
            .map(|&len| 1 << (FAST_CODE_BITS - len))
            .sum();
        assert_eq!(kraft, 1 << FAST_CODE_BITS);
    }

    fn check_writes<const N: usize>() {
        let mut cases: Vec<[u32; N]> = (0..300)
            .map(|seed| sample_counts::<N>(seed, 1 + (seed as usize * 13) % N))
            .collect();
        // Long runs of equal lengths and of zeros.
        cases.push([1; N]);
        let mut sparse = [0; N];
        for symbol in (0..N).step_by(11) {
            sparse[symbol] = 1 + symbol as u32;
        }
        cases.push(sparse);
        let mut ends = [0; N];
        ends[..3].fill(9);
        ends[N - 3..].fill(1);
        ends[N / 2..N / 2 + 8].fill(40);
        cases.push(ends);
        // A long zero run before a long run of equal lengths.
        let mut halves = [0; N];
        halves[0] = 1000;
        halves[N / 2..].fill(1);
        cases.push(halves);

        for (case, counts) in cases.iter().enumerate() {
            let mut fast = BitWriter::new();
            let (codes, bits) = write(&mut fast, counts).unwrap();
            let mut lengths_from_codes = [0_u8; N];
            for (len, &code) in lengths_from_codes.iter_mut().zip(&codes) {
                *len = if code == MISSING { 0 } else { code as u8 };
            }
            let cost: usize = counts
                .iter()
                .zip(&lengths_from_codes)
                .map(|(&count, &len)| count as usize * usize::from(len))
                .sum();
            assert_eq!(bits, cost, "N {N} case {case}");

            let used = counts.iter().filter(|&&count| count != 0).count();
            if used <= MAX_SIMPLE_PREFIX_SYMBOLS {
                continue;
            }
            let mut general = BitWriter::new();
            super::super::write_fast_complex_prefix_code_lengths(&mut general, &lengths_from_codes)
                .unwrap();
            assert_eq!(
                fast.written_bits(),
                general.written_bits(),
                "N {N} case {case}"
            );
            assert_eq!(fast.into_bytes(), general.into_bytes(), "N {N} case {case}");

            // Canonical codes, as the decoder assigns them.
            let mut expected = [MISSING_DENSE_SYMBOL_CODE; N];
            super::super::fill_dense_symbol_code_map_from_lengths(
                &lengths_from_codes,
                &mut expected,
            );
            for (symbol, (&code, expected)) in codes.iter().zip(&expected).enumerate() {
                assert_eq!(
                    (code as u8, (code >> 8) as u16),
                    (expected.len, expected.bits),
                    "N {N} case {case} symbol {symbol}"
                );
            }
        }
    }

    #[test]
    fn writes_same_bits_as_general_writer() {
        check_writes::<64>();
        check_writes::<LITERAL_ALPHABET_SIZE>();
        check_writes::<COMMAND_ALPHABET_SIZE>();
    }

    /// Reads back a complex prefix code's lengths, the way the decoder
    /// does.
    fn read_lengths<const N: usize>(bytes: &[u8]) -> [u8; N] {
        let mut reader = burli_core::bits::BitReader::new(bytes);
        let mut bit = || reader.read_bits(1).unwrap() as u8;
        let skip = usize::from(bit() | (bit() << 1));
        assert_ne!(skip, 1, "simple code");

        // Code-length code lengths, by the fixed code of their values.
        let mut code_lengths = [0_u8; CODE_LENGTH_ALPHABET_SIZE];
        let mut space = 32_i32;
        for &symbol in &CODE_LENGTH_ORDER[skip..] {
            let len = match (bit(), bit()) {
                (0, 0) => 0,
                (0, 1) => 3,
                (1, 0) => 4,
                (1, 1) => match bit() {
                    0 => 2,
                    _ => [1, 5][usize::from(bit())],
                },
                _ => unreachable!(),
            };
            code_lengths[usize::from(symbol)] = len;
            if len != 0 {
                space -= 32 >> len;
                if space <= 0 {
                    break;
                }
            }
        }
        assert_eq!(space, 0);

        // Canonical code-length codes, read MSB first.
        let mut next = [0_u32; 6];
        let mut code = 0;
        for (len, slot) in next.iter_mut().enumerate().skip(1) {
            let count = code_lengths
                .iter()
                .filter(|&&l| usize::from(l) == len - 1)
                .count();
            code = (code + if len == 1 { 0 } else { count as u32 }) << 1;
            *slot = code;
        }
        let mut table = Vec::new();
        for (symbol, &len) in code_lengths.iter().enumerate() {
            if len != 0 {
                table.push((len, next[usize::from(len)], symbol));
                next[usize::from(len)] += 1;
            }
        }
        let read_symbol = |bit: &mut dyn FnMut() -> u8| {
            let (mut len, mut code) = (0, 0);
            loop {
                code = (code << 1) | u32::from(bit());
                len += 1;
                if let Some(&(_, _, symbol)) =
                    table.iter().find(|&&(l, c, _)| l == len && c == code)
                {
                    return symbol;
                }
                assert!(len < 6);
            }
        };

        let mut lengths = [0_u8; N];
        let mut space = 1_i32 << 15;
        let mut symbol = 0;
        let mut previous = 8;
        let (mut repeat, mut repeat_code) = (0, 0);
        while space > 0 {
            let code = read_symbol(&mut bit);
            if code < 16 {
                lengths[symbol] = code as u8;
                symbol += 1;
                if code != 0 {
                    previous = code as u8;
                    space -= (1 << 15) >> code;
                }
                repeat_code = 0;
                continue;
            }
            let digit_bits = if code == 16 { 2 } else { 3 };
            let mut extra = 0;
            for i in 0..digit_bits {
                extra |= usize::from(bit()) << i;
            }
            if repeat_code != code {
                repeat = 0;
            }
            let old = repeat;
            if repeat > 0 {
                repeat = (repeat - 2) << digit_bits;
            }
            repeat += extra + 3;
            let value = if code == 16 { previous } else { 0 };
            for _ in old..repeat {
                lengths[symbol] = value;
                symbol += 1;
                if value != 0 {
                    space -= (1 << 15) >> value;
                }
            }
            repeat_code = code;
        }
        assert_eq!(space, 0);
        lengths
    }

    fn check_tuned<const N: usize>() {
        let mut cases: Vec<[u32; N]> = (0..300)
            .map(|seed| sample_counts::<N>(seed + 1000, 5 + (seed as usize * 17) % (N - 4)))
            .collect();
        cases.push([1; N]);
        let mut halves = [0; N];
        halves[0] = 1000;
        halves[N / 2..].fill(1);
        cases.push(halves);

        for (case, counts) in cases.iter().enumerate() {
            let mut tuned = BitWriter::new();
            let (codes, bits) = write_tuned(&mut tuned, counts).unwrap();
            let mut plain = BitWriter::new();
            let (plain_codes, plain_bits) = write(&mut plain, counts).unwrap();
            assert_eq!(
                (codes, bits),
                (plain_codes, plain_bits),
                "N {N} case {case}"
            );
            assert!(
                tuned.written_bits() <= plain.written_bits(),
                "N {N} case {case}"
            );
            let mut expected = [0_u8; N];
            for (len, &code) in expected.iter_mut().zip(&codes) {
                *len = if code == MISSING { 0 } else { code as u8 };
            }
            assert_eq!(
                read_lengths::<N>(&tuned.into_bytes()),
                expected,
                "N {N} case {case}"
            );
        }
    }

    #[test]
    fn tuned_lengths_read_back() {
        check_tuned::<64>();
        check_tuned::<LITERAL_ALPHABET_SIZE>();
        check_tuned::<COMMAND_ALPHABET_SIZE>();
    }
}
