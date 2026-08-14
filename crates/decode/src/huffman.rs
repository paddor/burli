use alloc::vec;
use alloc::vec::Vec;

use burli_core::{BurliError, DecompressError, bits::BitReader};

const MAX_CODE_BITS: u8 = 15;
const FAST_LOOKUP_BITS: u8 = 15;
const CODE_LENGTH_CODES: usize = 18;
const CODE_LENGTH_ORDER: [usize; CODE_LENGTH_CODES] =
    [1, 2, 3, 4, 0, 5, 17, 6, 16, 7, 8, 9, 10, 11, 12, 13, 14, 15];
const CODE_LENGTH_PREFIX_LEN: [u8; 16] = [2, 2, 2, 3, 2, 2, 2, 4, 2, 2, 2, 3, 2, 2, 2, 4];
const CODE_LENGTH_PREFIX_VALUE: [u8; 16] = [0, 4, 3, 2, 0, 4, 3, 1, 0, 4, 3, 2, 0, 4, 3, 5];
const REVERSE_BYTE: [u8; 256] = reverse_byte_table();

#[derive(Clone, Debug)]
pub(crate) struct PrefixCode {
    fast: Vec<Lookup>,
    fast_bits: u8,
    fast_mask: u64,
    single_symbol: Option<u16>,
    max_bits: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Entry {
    symbol: u16,
    len: u8,
    code: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Lookup(u16);

impl Lookup {
    const EMPTY: Self = Self(0);
    const SYMBOL_MASK: u16 = 0x0fff;

    const fn new(symbol: u16, len: u8) -> Self {
        Self(((len as u16) << 12) | symbol)
    }

    #[inline(always)]
    const fn len(self) -> u8 {
        (self.0 >> 12) as u8
    }

    #[inline(always)]
    const fn symbol(self) -> u16 {
        self.0 & Self::SYMBOL_MASK
    }
}

impl PrefixCode {
    pub(crate) fn single(symbol: u16) -> Self {
        Self {
            fast: Vec::new(),
            fast_bits: 0,
            fast_mask: 0,
            single_symbol: Some(symbol),
            max_bits: 0,
        }
    }

    #[inline(always)]
    pub(crate) const fn single_symbol(&self) -> Option<u16> {
        self.single_symbol
    }

    #[inline(always)]
    pub(crate) const fn max_bits(&self) -> u8 {
        self.max_bits
    }

    pub(crate) fn from_lengths(lengths: &[u8]) -> Result<Self, DecompressError> {
        let mut counts = [0_u16; MAX_CODE_BITS as usize + 1];
        let mut non_zero = 0_usize;
        let mut max_bits = 0_u8;

        for &len in lengths {
            if len > MAX_CODE_BITS {
                return Err(BurliError::Format("Brotli Huffman code length exceeds 15"));
            }
            if len != 0 {
                counts[usize::from(len)] += 1;
                non_zero += 1;
                max_bits = max_bits.max(len);
            }
        }

        if non_zero > 1 {
            validate_complete_counts(&counts)?;
        }
        Self::from_lengths_prechecked(lengths, counts, non_zero, max_bits)
    }

    fn from_lengths_prechecked(
        lengths: &[u8],
        counts: [u16; MAX_CODE_BITS as usize + 1],
        non_zero: usize,
        max_bits: u8,
    ) -> Result<Self, DecompressError> {
        if non_zero == 0 {
            return Err(BurliError::Format("empty Brotli Huffman code"));
        }
        if non_zero == 1 {
            let symbol = lengths
                .iter()
                .position(|&len| len != 0)
                .ok_or(BurliError::Format("empty Brotli Huffman code"))?;
            return Ok(Self::single(symbol as u16));
        }

        let mut next_code = [0_u16; MAX_CODE_BITS as usize + 1];
        let mut code = 0_u16;
        for bits in 1..=MAX_CODE_BITS {
            code = (code + counts[usize::from(bits - 1)]) << 1;
            next_code[usize::from(bits)] = code;
        }

        let fast_bits = max_bits.min(FAST_LOOKUP_BITS);
        let fast_mask = (1_u64 << fast_bits) - 1;
        let mut fast = vec![Lookup::EMPTY; 1 << fast_bits];
        for (symbol, &len) in lengths.iter().enumerate() {
            if len == 0 {
                continue;
            }
            let code = next_code[usize::from(len)];
            next_code[usize::from(len)] += 1;
            let entry = Entry {
                symbol: symbol as u16,
                len,
                code,
            };
            fill_fast_lookup(&mut fast, fast_bits, entry);
        }

        Ok(Self {
            fast,
            fast_bits,
            fast_mask,
            single_symbol: None,
            max_bits,
        })
    }

    fn from_simple_lengths(symbol_lengths: &mut [(usize, u8)]) -> Result<Self, DecompressError> {
        if symbol_lengths.is_empty() {
            return Err(BurliError::Format("empty Brotli Huffman code"));
        }
        symbol_lengths.sort_unstable_by_key(|&(symbol, _)| symbol);

        let mut counts = [0_u16; MAX_CODE_BITS as usize + 1];
        let mut max_bits = 0_u8;
        for &(symbol, len) in symbol_lengths.iter() {
            if symbol > usize::from(u16::MAX) {
                return Err(BurliError::Format("Brotli Huffman symbol exceeds range"));
            }
            if len == 0 || len > MAX_CODE_BITS {
                return Err(BurliError::Format("Brotli Huffman code length exceeds 15"));
            }
            counts[usize::from(len)] += 1;
            max_bits = max_bits.max(len);
        }

        if symbol_lengths.len() == 1 {
            return Ok(Self::single(symbol_lengths[0].0 as u16));
        }

        validate_complete_counts(&counts)?;

        let mut next_code = [0_u16; MAX_CODE_BITS as usize + 1];
        let mut code = 0_u16;
        for bits in 1..=MAX_CODE_BITS {
            code = (code + counts[usize::from(bits - 1)]) << 1;
            next_code[usize::from(bits)] = code;
        }

        let fast_bits = max_bits.min(FAST_LOOKUP_BITS);
        let fast_mask = (1_u64 << fast_bits) - 1;
        let mut fast = vec![Lookup::EMPTY; 1 << fast_bits];
        for &(symbol, len) in symbol_lengths.iter() {
            let code = next_code[usize::from(len)];
            next_code[usize::from(len)] += 1;
            let entry = Entry {
                symbol: symbol as u16,
                len,
                code,
            };
            fill_fast_lookup(&mut fast, fast_bits, entry);
        }

        Ok(Self {
            fast,
            fast_bits,
            fast_mask,
            single_symbol: None,
            max_bits,
        })
    }

    pub(crate) fn read(
        reader: &mut BitReader<'_>,
        alphabet_size: usize,
    ) -> Result<Self, DecompressError> {
        if alphabet_size == 0 || alphabet_size > usize::from(u16::MAX) + 1 {
            return Err(BurliError::Format("invalid Brotli Huffman alphabet size"));
        }

        let hskip = reader.read_bits(2)? as u8;
        if hskip == 1 {
            return read_simple_prefix_code(reader, alphabet_size);
        }
        read_complex_prefix_code(reader, alphabet_size, hskip)
    }

    #[inline(always)]
    pub(crate) fn decode(&self, reader: &mut BitReader<'_>) -> Result<u16, DecompressError> {
        if let Some(symbol) = self.single_symbol {
            return Ok(symbol);
        }

        self.decode_non_single(reader)
    }

    #[inline(always)]
    pub(crate) fn decode_non_single(
        &self,
        reader: &mut BitReader<'_>,
    ) -> Result<u16, DecompressError> {
        debug_assert!(self.single_symbol.is_none());

        if reader.has_bits(self.fast_bits) {
            let lookup = self.fast
                [reader.peek_bits_trusted_with_mask(self.fast_bits, self.fast_mask) as usize];
            debug_assert!(lookup.len() != 0);
            reader.drop_bits_trusted(lookup.len());
            return Ok(lookup.symbol());
        }

        self.decode_non_single_with_padded_lookup(reader)
    }

    #[inline(always)]
    pub(crate) fn decode_non_single_trusted_fast(&self, reader: &mut BitReader<'_>) -> u16 {
        debug_assert!(self.single_symbol.is_none());
        debug_assert!(reader.has_bits(self.fast_bits));

        let lookup =
            self.fast[reader.peek_bits_trusted_with_mask(self.fast_bits, self.fast_mask) as usize];
        debug_assert!(lookup.len() != 0);
        reader.drop_bits_trusted(lookup.len());
        lookup.symbol()
    }

    #[cold]
    #[inline(never)]
    fn decode_non_single_with_padded_lookup(
        &self,
        reader: &mut BitReader<'_>,
    ) -> Result<u16, DecompressError> {
        let remaining = reader.remaining_bits();
        if remaining == 0 {
            return Err(BurliError::Format("unexpected end of Brotli input"));
        }

        let available = remaining.min(usize::from(self.fast_bits));
        let index = reader.peek_bits(available as u8)? as usize;
        let lookup = self.fast[index];
        let len = lookup.len();
        if len == 0 {
            return Err(BurliError::Format("invalid Brotli Huffman code"));
        }
        if usize::from(len) > remaining {
            return Err(BurliError::Format("unexpected end of Brotli input"));
        }

        reader.drop_bits(len)?;
        Ok(lookup.symbol())
    }
}

fn validate_complete_counts(
    counts: &[u16; MAX_CODE_BITS as usize + 1],
) -> Result<(), DecompressError> {
    let mut space = 1_i32 << MAX_CODE_BITS;
    for bits in 1..=MAX_CODE_BITS {
        space -= i32::from(counts[usize::from(bits)]) << (MAX_CODE_BITS - bits);
        if space < 0 {
            return Err(BurliError::Format("oversubscribed Brotli Huffman code"));
        }
    }
    if space != 0 {
        return Err(BurliError::Format("incomplete Brotli Huffman code"));
    }
    Ok(())
}

fn fill_fast_lookup(fast: &mut [Lookup], fast_bits: u8, entry: Entry) {
    if entry.len > fast_bits || entry.symbol > Lookup::SYMBOL_MASK {
        return;
    }

    let prefix = reverse_low_bits(entry.code, entry.len);
    let step = 1_usize << entry.len;
    let mut index = usize::from(prefix);
    while index < fast.len() {
        fast[index] = Lookup::new(entry.symbol, entry.len);
        index += step;
    }
}

fn reverse_low_bits(value: u16, width: u8) -> u16 {
    let reversed = (u16::from(REVERSE_BYTE[usize::from(value & 0xff)]) << 8)
        | u16::from(REVERSE_BYTE[usize::from(value >> 8)]);
    reversed >> (u16::BITS as u8 - width)
}

const fn reverse_byte_table() -> [u8; 256] {
    let mut table = [0_u8; 256];
    let mut index = 0_usize;
    while index < table.len() {
        table[index] = (index as u8).reverse_bits();
        index += 1;
    }
    table
}

fn read_simple_prefix_code(
    reader: &mut BitReader<'_>,
    alphabet_size: usize,
) -> Result<PrefixCode, DecompressError> {
    let nsym = reader.read_bits(2)? as usize + 1;
    let alphabet_bits = alphabet_bits(alphabet_size);
    let mut symbols = Vec::with_capacity(nsym);
    for _ in 0..nsym {
        let symbol = reader.read_bits(alphabet_bits)? as usize;
        if symbol >= alphabet_size || symbols.contains(&symbol) {
            return Err(BurliError::Format("invalid Brotli simple Huffman symbol"));
        }
        symbols.push(symbol);
    }

    if nsym == 1 {
        return Ok(PrefixCode::single(symbols[0] as u16));
    }

    let mut symbol_lengths = [(0_usize, 0_u8); 4];
    match nsym {
        2 => {
            symbol_lengths[0] = (symbols[0], 1);
            symbol_lengths[1] = (symbols[1], 1);
        }
        3 => {
            symbol_lengths[0] = (symbols[0], 1);
            symbol_lengths[1] = (symbols[1], 2);
            symbol_lengths[2] = (symbols[2], 2);
        }
        4 => {
            if reader.read_bit()? {
                symbol_lengths[0] = (symbols[0], 1);
                symbol_lengths[1] = (symbols[1], 2);
                symbol_lengths[2] = (symbols[2], 3);
                symbol_lengths[3] = (symbols[3], 3);
            } else {
                for (slot, symbol) in symbol_lengths.iter_mut().zip(symbols) {
                    *slot = (symbol, 2);
                }
            }
        }
        _ => unreachable!(),
    }

    PrefixCode::from_simple_lengths(&mut symbol_lengths[..nsym])
}

fn alphabet_bits(alphabet_size: usize) -> u8 {
    let value = alphabet_size.saturating_sub(1);
    (usize::BITS - value.leading_zeros()) as u8
}

fn read_complex_prefix_code(
    reader: &mut BitReader<'_>,
    alphabet_size: usize,
    hskip: u8,
) -> Result<PrefixCode, DecompressError> {
    if hskip == 1 || hskip > 3 {
        return Err(BurliError::Format("invalid Brotli Huffman hskip"));
    }

    let mut code_length_code_lengths = [0_u8; CODE_LENGTH_CODES];
    let mut space = 32_i32;
    let mut non_zero = 0_usize;
    let start = if hskip == 0 { 0 } else { usize::from(hskip) };

    for &symbol in CODE_LENGTH_ORDER.iter().skip(start) {
        let len = read_code_length_code_len(reader)?;
        code_length_code_lengths[symbol] = len;
        if len != 0 {
            non_zero += 1;
            space -= 32_i32 >> len;
            if space < 0 {
                return Err(BurliError::Format(
                    "oversubscribed Brotli code-length Huffman code",
                ));
            }
            if non_zero >= 2 && space == 0 {
                break;
            }
        }
    }

    let code_length_code = if non_zero == 1 {
        let symbol = code_length_code_lengths
            .iter()
            .position(|&len| len != 0)
            .ok_or(BurliError::Format("empty Brotli code-length Huffman code"))?;
        PrefixCode::single(symbol as u16)
    } else {
        if space != 0 {
            return Err(BurliError::Format(
                "incomplete Brotli code-length Huffman code",
            ));
        }
        PrefixCode::from_lengths(&code_length_code_lengths)?
    };

    let code_lengths = read_symbol_code_lengths(reader, alphabet_size, &code_length_code)?;
    PrefixCode::from_lengths_prechecked(
        &code_lengths.lengths,
        code_lengths.counts,
        code_lengths.non_zero,
        code_lengths.max_bits,
    )
}

fn read_code_length_code_len(reader: &mut BitReader<'_>) -> Result<u8, DecompressError> {
    let available = reader.remaining_bits().min(4);
    if available < 2 {
        return Err(BurliError::Format("unexpected end of Brotli input"));
    }
    let index = reader.peek_bits(available as u8)? as usize;
    let len = CODE_LENGTH_PREFIX_LEN[index];
    if usize::from(len) > available {
        return Err(BurliError::Format("unexpected end of Brotli input"));
    }
    reader.drop_bits(len)?;
    Ok(CODE_LENGTH_PREFIX_VALUE[index])
}

#[derive(Clone, Debug)]
struct CodeLengths {
    lengths: Vec<u8>,
    counts: [u16; MAX_CODE_BITS as usize + 1],
    non_zero: usize,
    max_bits: u8,
}

#[derive(Clone, Debug)]
struct CodeLengthStats {
    counts: [u16; MAX_CODE_BITS as usize + 1],
    non_zero: usize,
    max_bits: u8,
}

fn read_symbol_code_lengths(
    reader: &mut BitReader<'_>,
    alphabet_size: usize,
    code_length_code: &PrefixCode,
) -> Result<CodeLengths, DecompressError> {
    let mut lengths = vec![0_u8; alphabet_size];
    let mut stats = CodeLengthStats {
        counts: [0_u16; MAX_CODE_BITS as usize + 1],
        non_zero: 0,
        max_bits: 0,
    };
    let mut cursor = 0_usize;
    let mut space = 1_i32 << MAX_CODE_BITS;
    let mut previous_non_zero = 8_u8;
    let mut pending_repeat: Option<Repeat> = None;

    while cursor < alphabet_size && space > 0 {
        if pending_repeat_finishes_tree(alphabet_size, cursor, space, pending_repeat)? {
            apply_pending_repeat(
                &mut lengths,
                &mut cursor,
                &mut space,
                &mut stats,
                &mut previous_non_zero,
                &mut pending_repeat,
            )?;
            continue;
        }

        let symbol = code_length_code.decode(reader)? as u8;
        if symbol <= 15 {
            apply_pending_repeat(
                &mut lengths,
                &mut cursor,
                &mut space,
                &mut stats,
                &mut previous_non_zero,
                &mut pending_repeat,
            )?;
            if cursor >= alphabet_size {
                return Err(BurliError::Format("Brotli code length exceeds alphabet"));
            }
            lengths[cursor] = symbol;
            cursor += 1;
            if symbol != 0 {
                previous_non_zero = symbol;
                space -= 1_i32 << (MAX_CODE_BITS - symbol);
                if space < 0 {
                    return Err(BurliError::Format("oversubscribed Brotli Huffman code"));
                }
                stats.counts[usize::from(symbol)] += 1;
                stats.non_zero += 1;
                stats.max_bits = stats.max_bits.max(symbol);
            }
            continue;
        }

        let repeat = read_repeat(reader, symbol, previous_non_zero)?;
        match pending_repeat.as_mut() {
            Some(pending) if pending.code == repeat.code => {
                pending.count = repeat.factor * (pending.count - 2) + repeat.count;
            }
            _ => {
                apply_pending_repeat(
                    &mut lengths,
                    &mut cursor,
                    &mut space,
                    &mut stats,
                    &mut previous_non_zero,
                    &mut pending_repeat,
                )?;
                pending_repeat = Some(repeat);
            }
        }
    }

    apply_pending_repeat(
        &mut lengths,
        &mut cursor,
        &mut space,
        &mut stats,
        &mut previous_non_zero,
        &mut pending_repeat,
    )?;

    if space != 0 {
        return Err(BurliError::Format("incomplete Brotli Huffman code"));
    }

    lengths.truncate(cursor);

    Ok(CodeLengths {
        lengths,
        counts: stats.counts,
        non_zero: stats.non_zero,
        max_bits: stats.max_bits,
    })
}

#[derive(Clone, Copy, Debug)]
struct Repeat {
    code: u8,
    value: u8,
    count: usize,
    factor: usize,
}

fn read_repeat(
    reader: &mut BitReader<'_>,
    symbol: u8,
    previous_non_zero: u8,
) -> Result<Repeat, DecompressError> {
    match symbol {
        16 => Ok(Repeat {
            code: 16,
            value: previous_non_zero,
            count: reader.read_bits(2)? as usize + 3,
            factor: 4,
        }),
        17 => Ok(Repeat {
            code: 17,
            value: 0,
            count: reader.read_bits(3)? as usize + 3,
            factor: 8,
        }),
        _ => Err(BurliError::Format("invalid Brotli code-length repeat code")),
    }
}

fn apply_pending_repeat(
    lengths: &mut [u8],
    cursor: &mut usize,
    space: &mut i32,
    stats: &mut CodeLengthStats,
    previous_non_zero: &mut u8,
    pending_repeat: &mut Option<Repeat>,
) -> Result<(), DecompressError> {
    let Some(repeat) = pending_repeat.take() else {
        return Ok(());
    };

    let end = cursor
        .checked_add(repeat.count)
        .ok_or(BurliError::Format("Brotli code length repeat overflow"))?;
    if end > lengths.len() {
        return Err(BurliError::Format(
            "Brotli code length repeat exceeds alphabet",
        ));
    }

    for len in &mut lengths[*cursor..end] {
        *len = repeat.value;
    }
    *cursor = end;

    if repeat.value != 0 {
        *previous_non_zero = repeat.value;
        *space -= (repeat.count as i32) << (MAX_CODE_BITS - repeat.value);
        if *space < 0 {
            return Err(BurliError::Format("oversubscribed Brotli Huffman code"));
        }
        let count = u16::try_from(repeat.count)
            .map_err(|_| BurliError::Format("Brotli code length repeat exceeds alphabet"))?;
        let slot = &mut stats.counts[usize::from(repeat.value)];
        *slot = slot
            .checked_add(count)
            .ok_or(BurliError::Format("Brotli code length repeat overflow"))?;
        stats.non_zero += repeat.count;
        stats.max_bits = stats.max_bits.max(repeat.value);
    }

    Ok(())
}

fn pending_repeat_finishes_tree(
    alphabet_size: usize,
    cursor: usize,
    space: i32,
    pending_repeat: Option<Repeat>,
) -> Result<bool, DecompressError> {
    let Some(repeat) = pending_repeat else {
        return Ok(false);
    };
    let end = cursor
        .checked_add(repeat.count)
        .ok_or(BurliError::Format("Brotli code length repeat overflow"))?;
    if end > alphabet_size {
        return Err(BurliError::Format(
            "Brotli code length repeat exceeds alphabet",
        ));
    }
    if end == alphabet_size {
        return Ok(true);
    }
    if repeat.value == 0 {
        return Ok(false);
    }

    let remaining = space - ((repeat.count as i32) << (MAX_CODE_BITS - repeat.value));
    Ok(remaining == 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use burli_core::bits::BitWriter;

    #[test]
    fn canonical_code_decodes_symbols() {
        let code = PrefixCode::from_lengths(&[2, 1, 3, 3]).unwrap();
        let mut bits = BitWriter::new();
        bits.write_bits(1, 0b0).unwrap();
        bits.write_bits(2, 0b01).unwrap();
        bits.write_bits(3, 0b011).unwrap();
        bits.write_bits(3, 0b111).unwrap();
        let bytes = bits.into_bytes();
        let mut reader = BitReader::new(&bytes);

        assert_eq!(code.decode(&mut reader).unwrap(), 1);
        assert_eq!(code.decode(&mut reader).unwrap(), 0);
        assert_eq!(code.decode(&mut reader).unwrap(), 2);
        assert_eq!(code.decode(&mut reader).unwrap(), 3);
    }

    #[test]
    fn dense_lengths_still_reject_invalid_codes() {
        assert!(matches!(
            PrefixCode::from_lengths(&[1, 1, 1]),
            Err(BurliError::Format("oversubscribed Brotli Huffman code"))
        ));
        assert!(matches!(
            PrefixCode::from_lengths(&[2, 2]),
            Err(BurliError::Format("incomplete Brotli Huffman code"))
        ));
    }

    #[test]
    fn simple_code_with_one_symbol_reads_no_payload_bits() {
        let mut bits = BitWriter::new();
        bits.write_bits(2, 0b01).unwrap();
        bits.write_bits(2, 0).unwrap();
        bits.write_bits(8, 42).unwrap();
        bits.write_bits(3, 0b111).unwrap();
        let bytes = bits.into_bytes();
        let mut reader = BitReader::new(&bytes);
        let code = PrefixCode::read(&mut reader, 256).unwrap();

        assert_eq!(code.decode(&mut reader).unwrap(), 42);
        assert_eq!(reader.read_bits(3).unwrap(), 0b111);
    }

    #[test]
    fn simple_code_rejects_duplicate_symbols() {
        let mut bits = BitWriter::new();
        bits.write_bits(2, 0b01).unwrap();
        bits.write_bits(2, 1).unwrap();
        bits.write_bits(8, 7).unwrap();
        bits.write_bits(8, 7).unwrap();
        let bytes = bits.into_bytes();
        let mut reader = BitReader::new(&bytes);

        assert!(matches!(
            PrefixCode::read(&mut reader, 256),
            Err(BurliError::Format("invalid Brotli simple Huffman symbol"))
        ));
    }
}

#[cfg(kani)]
mod verification {
    use super::*;

    #[kani::proof]
    fn single_symbol_code_decodes_without_input_bits() {
        let symbol = kani::any::<u8>();
        let code = PrefixCode::single(u16::from(symbol));
        let mut reader = BitReader::new(&[]);

        assert_eq!(code.decode(&mut reader).unwrap(), u16::from(symbol));
        assert_eq!(reader.consumed_bits(), 0);
    }
}
