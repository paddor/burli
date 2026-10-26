use alloc::vec;
use alloc::vec::Vec;

use burli_core::{BurliError, DecompressError, bits::BitReader};

const MAX_CODE_BITS: u8 = 15;
/// Index bits of a root table. Longer codes continue in a second-level table.
const ROOT_BITS: u8 = 8;
pub(crate) const ROOT_SIZE: usize = 1 << ROOT_BITS;
/// Index bits of the root table a group of one literal code adds when its
/// codes run longer than [`ROOT_BITS`]. Binary data has many 9- to 11-bit
/// literal codes, and each second-level lookup costs a mispredicted branch.
const WIDE_ROOT_BITS: u8 = 10;
pub(crate) const WIDE_ROOT_SIZE: usize = 1 << WIDE_ROOT_BITS;
/// The most symbols of any Brotli alphabet: the command alphabet.
const MAX_ALPHABET_SIZE: usize = 704;
/// Share of the code space, in 1/32768ths, that codes longer than
/// [`ROOT_BITS`] must exceed before a group of one code builds the wide root.
/// Text gives long codes to rare symbols only, and a wider root just costs
/// building and cache space there.
const WIDE_ROOT_MIN_SHARE: u32 = 1 << (MAX_CODE_BITS - 3);
/// Index bits of the code-length code's table. Its codes have at most 5 bits.
const LENGTH_CODE_BITS: u8 = 5;
const CODE_LENGTH_CODES: usize = 18;
const CODE_LENGTH_ORDER: [usize; CODE_LENGTH_CODES] =
    [1, 2, 3, 4, 0, 5, 17, 6, 16, 7, 8, 9, 10, 11, 12, 13, 14, 15];
const CODE_LENGTH_PREFIX_LEN: [u8; 16] = [2, 2, 2, 3, 2, 2, 2, 4, 2, 2, 2, 3, 2, 2, 2, 4];
const CODE_LENGTH_PREFIX_VALUE: [u8; 16] = [0, 4, 3, 2, 0, 4, 3, 1, 0, 4, 3, 2, 0, 4, 3, 5];
const REVERSE_BYTE: [u8; 256] = reverse_byte_table();

/// One table entry. A leaf holds a symbol and its code length. A root entry
/// for codes longer than [`ROOT_BITS`] holds the offset of their
/// second-level table in the group buffer, and `ROOT_BITS` plus that
/// table's index bits.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Lookup(u32);

impl Lookup {
    const EMPTY: Self = Self(0);

    const fn new(value: usize, len: u8) -> Self {
        debug_assert!(value < 1 << 24);
        Self(((value as u32) << 8) | len as u32)
    }

    #[inline(always)]
    const fn value(self) -> usize {
        (self.0 >> 8) as usize
    }

    #[inline(always)]
    const fn len(self) -> u8 {
        self.0 as u8
    }
}

/// The prefix codes of one alphabet in a meta-block header, in one buffer:
/// a root table of [`ROOT_SIZE`] entries per code, then the second-level
/// tables. A single-symbol code fills its root with zero-length entries, so
/// it decodes like any other code.
#[derive(Clone, Debug)]
pub(crate) struct CodeGroup {
    entries: Vec<Lookup>,
    count: usize,
    /// Longest code length in the group.
    max_bits: u8,
    /// Whether a group of one code builds `wide` as well.
    wants_wide: bool,
    /// A root of [`WIDE_ROOT_SIZE`] entries and its second-level tables for
    /// the one code, or empty.
    wide: Vec<Lookup>,
}

impl CodeGroup {
    /// Reads `count` prefix codes over `alphabet_size` symbols. With `wide`,
    /// a group of one code with codes longer than [`ROOT_BITS`] also builds a
    /// [`WIDE_ROOT_SIZE`] root.
    pub(crate) fn read(
        reader: &mut BitReader<'_>,
        count: usize,
        alphabet_size: usize,
        wide: bool,
    ) -> Result<Self, DecompressError> {
        if alphabet_size == 0 || alphabet_size > MAX_ALPHABET_SIZE {
            return Err(BurliError::Format("invalid Brotli Huffman alphabet size"));
        }
        let mut group = Self::with_count(count, wide && count == 1);
        let mut lists = SymbolLists::new();
        for tree in 0..count {
            let hskip = reader.read_bits(2)? as u8;
            if hskip == 1 {
                read_simple_prefix_code(reader, alphabet_size, &mut lists)?;
            } else {
                read_complex_prefix_code(reader, alphabet_size, hskip, &mut lists)?;
            }
            group.set_code(tree, &lists)?;
        }
        Ok(group)
    }

    fn with_count(count: usize, wants_wide: bool) -> Self {
        Self {
            entries: vec![Lookup::EMPTY; count * ROOT_SIZE],
            count,
            max_bits: 0,
            wants_wide,
            wide: Vec::new(),
        }
    }

    /// The tables of the one code with a [`WIDE_ROOT_SIZE`] root, when the
    /// group built them.
    #[inline(always)]
    pub(crate) fn wide_table(&self) -> Option<Table<'_, WIDE_ROOT_SIZE>> {
        self.wide.first_chunk::<WIDE_ROOT_SIZE>().map(|root| Table {
            root,
            entries: &self.wide,
        })
    }

    /// Every table of the group, borrowed for decoding.
    #[inline(always)]
    pub(crate) fn tables(&self) -> Tables<'_> {
        Tables {
            roots: self.entries[..self.count * ROOT_SIZE]
                .as_chunks::<ROOT_SIZE>()
                .0,
            entries: &self.entries,
        }
    }

    pub(crate) const fn len(&self) -> usize {
        self.count
    }

    /// Longest code length in the group, zero when every code has a single
    /// symbol.
    pub(crate) const fn max_bits(&self) -> u8 {
        self.max_bits
    }

    pub(crate) fn single_symbol(&self, tree: usize) -> Option<u16> {
        // Built codes have lengths of at least one bit, so a zero-length
        // first entry marks a single-symbol code.
        let first = self.entries.get(tree * ROOT_SIZE)?;
        (first.len() == 0).then(|| first.value() as u16)
    }

    /// Builds the tables of code `tree` from `lists`, which hold a complete
    /// code or one symbol.
    fn set_code(&mut self, tree: usize, lists: &SymbolLists) -> Result<(), DecompressError> {
        let base = tree * ROOT_SIZE;
        match lists.total {
            0 => Err(BurliError::Format("empty Brotli Huffman code")),
            1 => {
                let symbol = lists.only_symbol();
                self.entries[base..base + ROOT_SIZE].fill(Lookup::new(symbol, 0));
                Ok(())
            }
            _ => {
                let max_bits = lists.max_bits();
                build_code::<ROOT_SIZE>(&mut self.entries, base, lists, max_bits);
                if self.wants_wide && long_code_share(&lists.counts) > WIDE_ROOT_MIN_SHARE {
                    self.wide = vec![Lookup::EMPTY; WIDE_ROOT_SIZE];
                    build_code::<WIDE_ROOT_SIZE>(&mut self.wide, 0, lists, max_bits);
                }
                self.max_bits = self.max_bits.max(max_bits);
                Ok(())
            }
        }
    }
}

/// Slots of [`SymbolLists::links`] that head the list of each code length.
const LIST_HEADS: usize = MAX_CODE_BITS as usize + 1;

/// The symbols of one code with a nonzero length, as one linked list per
/// length in increasing symbol order. Canonical order is by length, then by
/// symbol, so building a table walks the lists without sorting.
struct SymbolLists {
    /// Slot `len` links to the first symbol of length `len`, and slot
    /// `LIST_HEADS + symbol` to the next symbol of the same length. Each
    /// list is as long as its count, so it needs no terminator.
    links: [u16; LIST_HEADS + MAX_ALPHABET_SIZE],
    /// The last slot of each list.
    tails: [usize; MAX_CODE_BITS as usize + 1],
    counts: [u16; MAX_CODE_BITS as usize + 1],
    total: usize,
}

impl SymbolLists {
    fn new() -> Self {
        Self {
            links: [0; LIST_HEADS + MAX_ALPHABET_SIZE],
            tails: core::array::from_fn(|len| len),
            counts: [0; MAX_CODE_BITS as usize + 1],
            total: 0,
        }
    }

    fn clear(&mut self) {
        self.tails = core::array::from_fn(|len| len);
        self.counts = [0; MAX_CODE_BITS as usize + 1];
        self.total = 0;
    }

    /// Appends `symbol` to the list of `len`. Symbols arrive in increasing
    /// order.
    #[inline(always)]
    fn push(&mut self, symbol: usize, len: u8) {
        let len = usize::from(len);
        let slot = LIST_HEADS + symbol;
        self.links[self.tails[len]] = slot as u16;
        self.tails[len] = slot;
        self.counts[len] += 1;
        self.total += 1;
    }

    /// Appends the symbols `start..end` to the list of `len`.
    fn push_run(&mut self, start: usize, end: usize, len: u8) {
        debug_assert!(start < end);
        let len = usize::from(len);
        let (first, last) = (LIST_HEADS + start, LIST_HEADS + end - 1);
        self.links[self.tails[len]] = first as u16;
        for (slot, link) in (first + 1..).zip(&mut self.links[first..last]) {
            *link = slot as u16;
        }
        self.tails[len] = last;
        self.counts[len] += (end - start) as u16;
        self.total += end - start;
    }

    /// The symbols of length `len` in increasing order.
    #[inline(always)]
    fn symbols(&self, len: usize) -> impl Iterator<Item = usize> + '_ {
        let mut slot = len;
        (0..self.counts[len]).map(move |_| {
            slot = usize::from(self.links[slot]);
            slot - LIST_HEADS
        })
    }

    /// The symbol of a code of one symbol.
    fn only_symbol(&self) -> usize {
        let len = self
            .counts
            .iter()
            .position(|&count| count != 0)
            .unwrap_or(0);
        usize::from(self.links[len]).saturating_sub(LIST_HEADS)
    }

    fn max_bits(&self) -> u8 {
        self.counts
            .iter()
            .rposition(|&count| count != 0)
            .unwrap_or(0) as u8
    }
}

/// Builds the tables of a complete code into `entries`: a root of `ROOT`
/// entries at `base`, and second-level tables appended at the end.
fn build_code<const ROOT: usize>(
    entries: &mut Vec<Lookup>,
    base: usize,
    lists: &SymbolLists,
    max_bits: u8,
) {
    debug_assert!((1..=MAX_CODE_BITS).contains(&max_bits));
    let root_bits = ROOT.trailing_zeros() as usize;
    let max_bits = usize::from(max_bits);

    // Canonical codes count up within a length and double between lengths.
    // The root fills to the width of its longest code, then repeats.
    let table_bits = max_bits.min(root_bits);
    let root: &mut [Lookup; ROOT] = (&mut entries[base..base + ROOT])
        .try_into()
        .expect("root has ROOT entries");
    let filled = &mut root[..1 << table_bits];
    let mut code = 0_usize;
    for len in 1..=table_bits {
        for symbol in lists.symbols(len) {
            let entry = Lookup::new(symbol, len as u8);
            let mut slot = if ROOT <= ROOT_SIZE {
                reverse_short(code, len)
            } else {
                usize::from(reverse_low_bits(code as u16, len as u8))
            };
            while slot < filled.len() {
                filled[slot] = entry;
                slot += 1 << len;
            }
            code += 1;
        }
        code <<= 1;
    }
    let mut size = 1 << table_bits;
    while size < ROOT {
        root.copy_within(..size, size);
        size <<= 1;
    }

    if max_bits <= root_bits {
        return;
    }
    // The long codes of one root prefix are consecutive and form a complete
    // subcode. Its longest code sets the size of its second-level table.
    // Sizing the tables first takes one resize for all of them.
    let second_base = entries.len();
    let second_size = second_level_size(&lists.counts, code, root_bits, max_bits);
    entries.resize(second_base + second_size, Lookup::EMPTY);
    let (front, second) = entries.split_at_mut(second_base);
    let root = &mut front[base..base + ROOT];
    let mut remaining = lists.counts;
    let mut prefix = usize::MAX;
    let mut table: &mut [Lookup] = &mut [];
    let mut end = 0;
    for len in root_bits + 1..=max_bits {
        let sub_bits = len - root_bits;
        for (placed, symbol) in lists.symbols(len).enumerate() {
            if code >> sub_bits != prefix {
                prefix = code >> sub_bits;
                remaining[len] = lists.counts[len] - placed as u16;
                let bits = next_table_bits(&remaining, len, root_bits);
                let start = end;
                end += 1 << bits;
                let key = if ROOT <= ROOT_SIZE {
                    reverse_short(prefix, root_bits)
                } else {
                    usize::from(reverse_low_bits(prefix as u16, root_bits as u8))
                };
                root[key] = Lookup::new(second_base + start, (root_bits + bits) as u8);
                table = &mut second[start..end];
            }
            let entry = Lookup::new(symbol, len as u8);
            let mut slot = reverse_short(code & ((1 << sub_bits) - 1), sub_bits);
            while slot < table.len() {
                table[slot] = entry;
                slot += 1 << sub_bits;
            }
            code += 1;
        }
        code <<= 1;
    }
}

/// Every table of a [`CodeGroup`], borrowed for decoding.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Tables<'a> {
    roots: &'a [[Lookup; ROOT_SIZE]],
    /// The whole group buffer, which root entries address absolutely.
    entries: &'a [Lookup],
}

impl<'a> Tables<'a> {
    /// The tables of code `tree`. Only selecting the root is bounds checked.
    #[inline(always)]
    pub(crate) fn table(self, tree: usize) -> Table<'a> {
        Table {
            root: &self.roots[tree],
            entries: self.entries,
        }
    }
}

/// The tables of one prefix code, with a root of `N` entries.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Table<'a, const N: usize = ROOT_SIZE> {
    root: &'a [Lookup; N],
    entries: &'a [Lookup],
}

impl<const N: usize> Table<'_, N> {
    const ROOT_BITS: u8 = N.trailing_zeros() as u8;

    /// Decodes one symbol. Fails only when the input ends inside the code.
    #[inline(always)]
    pub(crate) fn decode(self, reader: &mut BitReader<'_>) -> Result<u16, DecompressError> {
        reader.fill(MAX_CODE_BITS);
        self.decode_refilled(reader)
    }

    /// Decodes one symbol right after [`BitReader::refill`]. The refill
    /// buffers 15 bits or all remaining input, so only a truncated code
    /// fails.
    #[inline(always)]
    pub(crate) fn decode_refilled(
        self,
        reader: &mut BitReader<'_>,
    ) -> Result<u16, DecompressError> {
        let lookup = self.lookup(reader.buffer());
        let len = lookup.len();
        if u32::from(len) > reader.buffered_bits() {
            return Err(BurliError::Format("unexpected end of Brotli input"));
        }
        reader.drop_bits_trusted(len);
        Ok(lookup.value() as u16)
    }

    /// Decodes one symbol from the buffered bits without refilling. The
    /// caller proves that the buffer holds this symbol's code.
    #[inline(always)]
    pub(crate) fn decode_buffered(self, reader: &mut BitReader<'_>) -> u16 {
        let lookup = self.lookup(reader.buffer());
        reader.drop_bits_trusted(lookup.len());
        lookup.value() as u16
    }

    /// Decodes one symbol. The caller proves that the input holds it.
    #[inline(always)]
    pub(crate) fn decode_trusted(self, reader: &mut BitReader<'_>) -> u16 {
        reader.fill(MAX_CODE_BITS);
        self.decode_buffered(reader)
    }

    /// Decodes one symbol from the low bits of `bits` and returns it with its
    /// code length.
    #[inline(always)]
    pub(crate) fn lookup_bits(self, bits: u64) -> (u16, u8) {
        let lookup = self.lookup(bits);
        (lookup.value() as u16, lookup.len())
    }

    /// Resolves the entry for the low bits of `bits`. Bits past the input end
    /// must be zero.
    #[inline(always)]
    fn lookup(self, bits: u64) -> Lookup {
        let root = self.root[bits as usize & (N - 1)];
        if root.len() <= Self::ROOT_BITS {
            return root;
        }
        let sub_mask = (1_usize << (root.len() - Self::ROOT_BITS)) - 1;
        // The builder sizes each second-level table for every masked index,
        // so the fallback never runs. It keeps a panic path out of the decode
        // loops.
        self.entries
            .get(root.value() + ((bits >> Self::ROOT_BITS) as usize & sub_mask))
            .copied()
            .unwrap_or(Lookup::EMPTY)
    }
}

/// One prefix code outside the literal, command, and distance groups: block
/// type and count codes and context map codes.
#[derive(Clone, Debug)]
pub(crate) struct PrefixCode(CodeGroup);

impl PrefixCode {
    pub(crate) fn read(
        reader: &mut BitReader<'_>,
        alphabet_size: usize,
    ) -> Result<Self, DecompressError> {
        CodeGroup::read(reader, 1, alphabet_size, false).map(Self)
    }

    #[inline(always)]
    pub(crate) fn table(&self) -> Table<'_> {
        self.0.tables().table(0)
    }

    #[inline(always)]
    pub(crate) fn decode(&self, reader: &mut BitReader<'_>) -> Result<u16, DecompressError> {
        self.table().decode(reader)
    }

    #[cfg(test)]
    pub(crate) fn from_lengths(lengths: &[u8]) -> Result<Self, DecompressError> {
        let mut lists = SymbolLists::new();
        for (symbol, &len) in lengths.iter().enumerate() {
            if len > MAX_CODE_BITS {
                return Err(BurliError::Format("Brotli Huffman code length exceeds 15"));
            }
            if len != 0 {
                lists.push(symbol, len);
            }
        }
        if lists.total > 1 {
            validate_complete_counts(&lists.counts)?;
        }
        let mut group = CodeGroup::with_count(1, false);
        group.set_code(0, &lists)?;
        Ok(Self(group))
    }
}

/// The code-length code of a complex prefix code: 18 symbols of at most 5
/// bits, in one table on the stack.
struct LengthCode([Lookup; 1 << LENGTH_CODE_BITS]);

impl LengthCode {
    /// Builds the table from lengths that form a complete code, or that hold
    /// one symbol, which decodes with zero bits.
    fn new(lengths: &[u8; CODE_LENGTH_CODES], non_zero: usize) -> Self {
        let mut table = [Lookup::EMPTY; 1 << LENGTH_CODE_BITS];
        if non_zero == 1 {
            let symbol = lengths.iter().position(|&len| len != 0).unwrap_or(0);
            table.fill(Lookup::new(symbol, 0));
            return Self(table);
        }
        let mut counts = [0_u16; MAX_CODE_BITS as usize + 1];
        for &len in lengths {
            if len != 0 {
                counts[usize::from(len)] += 1;
            }
        }
        let mut next_code = first_codes(&counts);
        for (symbol, &len) in lengths.iter().enumerate() {
            if len == 0 {
                continue;
            }
            let code = next_code[usize::from(len)];
            next_code[usize::from(len)] += 1;
            let entry = Lookup::new(symbol, len);
            let mut index = usize::from(reverse_low_bits(code, len));
            while index < table.len() {
                table[index] = entry;
                index += 1 << len;
            }
        }
        Self(table)
    }

    fn decode(&self, reader: &mut BitReader<'_>) -> Result<u8, DecompressError> {
        reader.fill(LENGTH_CODE_BITS);
        let lookup = self.0[(reader.buffer() & ((1 << LENGTH_CODE_BITS) - 1)) as usize];
        if u32::from(lookup.len()) > reader.buffered_bits() {
            return Err(BurliError::Format("unexpected end of Brotli input"));
        }
        reader.drop_bits_trusted(lookup.len());
        Ok(lookup.value() as u8)
    }
}

/// The share of the code space, in 1/32768ths, that codes longer than
/// [`ROOT_BITS`] take. A complete code decodes a symbol of length `len`
/// with probability `2^-len`, so this is the expected share of second-level
/// lookups.
fn long_code_share(counts: &[u16; MAX_CODE_BITS as usize + 1]) -> u32 {
    (ROOT_BITS + 1..=MAX_CODE_BITS)
        .map(|len| u32::from(counts[usize::from(len)]) << (MAX_CODE_BITS - len))
        .sum()
}

/// The index bits of the second-level table whose first code has length
/// `len`, given the codes of each length not yet placed. The table ends
/// where those codes fill the subtree of its root prefix.
fn next_table_bits(
    remaining: &[u16; MAX_CODE_BITS as usize + 1],
    mut len: usize,
    root_bits: usize,
) -> usize {
    let mut left = 1_i32 << (len - root_bits);
    while len < usize::from(MAX_CODE_BITS) {
        left -= i32::from(remaining[len]);
        if left <= 0 {
            break;
        }
        len += 1;
        left <<= 1;
    }
    len - root_bits
}

/// The second-level entries of a complete code with these length counts and
/// a root of `root_bits`. `code` is the first code of length `root_bits + 1`.
/// Replays the canonical walk of [`build_code`]: a table starts where the
/// root prefix changes, at the width [`next_table_bits`] gives.
fn second_level_size(
    counts: &[u16; MAX_CODE_BITS as usize + 1],
    mut code: usize,
    root_bits: usize,
    max_bits: usize,
) -> usize {
    let mut remaining = *counts;
    let mut prefix = usize::MAX;
    let mut size = 0;
    for len in root_bits + 1..=max_bits {
        let sub_bits = len - root_bits;
        let end = code + usize::from(counts[len]);
        while code < end {
            if code >> sub_bits != prefix {
                prefix = code >> sub_bits;
                remaining[len] = (end - code) as u16;
                size += 1 << next_table_bits(&remaining, len, root_bits);
            }
            // Skip the other codes of this length under the prefix.
            code = end.min((prefix + 1) << sub_bits);
        }
        code <<= 1;
    }
    size
}

/// The first canonical code of each length.
fn first_codes(counts: &[u16; MAX_CODE_BITS as usize + 1]) -> [u16; MAX_CODE_BITS as usize + 1] {
    let mut first_code = [0_u16; MAX_CODE_BITS as usize + 1];
    let mut code = 0_u16;
    for bits in 1..=MAX_CODE_BITS {
        code = (code + counts[usize::from(bits - 1)]) << 1;
        first_code[usize::from(bits)] = code;
    }
    first_code
}

#[cfg(test)]
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

/// `value` of `width` bits, at most eight, with its bits reversed. Table
/// indexes are reversed codes, since the stream delivers the first code bit
/// lowest.
#[inline(always)]
fn reverse_short(value: usize, width: usize) -> usize {
    debug_assert!(width <= 8 && value < 1 << width);
    usize::from(REVERSE_BYTE[value & 0xff]) >> (8 - width)
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

/// Reads a simple prefix code into `lists`.
fn read_simple_prefix_code(
    reader: &mut BitReader<'_>,
    alphabet_size: usize,
    lists: &mut SymbolLists,
) -> Result<(), DecompressError> {
    let nsym = reader.read_bits(2)? as usize + 1;
    let alphabet_bits = alphabet_bits(alphabet_size);
    let mut symbols = [0_usize; 4];
    for index in 0..nsym {
        let symbol = reader.read_bits(alphabet_bits)? as usize;
        if symbol >= alphabet_size || symbols[..index].contains(&symbol) {
            return Err(BurliError::Format("invalid Brotli simple Huffman symbol"));
        }
        symbols[index] = symbol;
    }

    let lengths: [u8; 4] = match nsym {
        1 => [0, 0, 0, 0],
        2 => [1, 1, 0, 0],
        3 => [1, 2, 2, 0],
        _ if reader.read_bit()? => [1, 2, 3, 3],
        _ => [2, 2, 2, 2],
    };
    let mut pairs = [(0_usize, 0_u8); 4];
    for (pair, (&symbol, &len)) in pairs.iter_mut().zip(symbols.iter().zip(&lengths)) {
        *pair = (symbol, len);
    }
    // Lists take their symbols in increasing order.
    let pairs = &mut pairs[..nsym];
    pairs.sort_unstable_by_key(|&(symbol, _)| symbol);
    lists.clear();
    for &(symbol, len) in pairs.iter() {
        // A code of one symbol has no length; any list holds it.
        lists.push(symbol, len.max(1));
    }
    Ok(())
}

fn alphabet_bits(alphabet_size: usize) -> u8 {
    let value = alphabet_size.saturating_sub(1);
    (usize::BITS - value.leading_zeros()) as u8
}

/// Reads a complex prefix code into `lists`.
fn read_complex_prefix_code(
    reader: &mut BitReader<'_>,
    alphabet_size: usize,
    hskip: u8,
    lists: &mut SymbolLists,
) -> Result<(), DecompressError> {
    if hskip > 3 {
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

    if non_zero != 1 && space != 0 {
        return Err(BurliError::Format(
            "incomplete Brotli code-length Huffman code",
        ));
    }
    let code_length_code = LengthCode::new(&code_length_code_lengths, non_zero);

    read_symbol_code_lengths(reader, alphabet_size, &code_length_code, lists)
}

fn read_code_length_code_len(reader: &mut BitReader<'_>) -> Result<u8, DecompressError> {
    reader.fill(4);
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

/// Reads the code length of each symbol into `lists`, until the lengths
/// form a complete code.
fn read_symbol_code_lengths(
    reader: &mut BitReader<'_>,
    alphabet_size: usize,
    code_length_code: &LengthCode,
    lists: &mut SymbolLists,
) -> Result<(), DecompressError> {
    lists.clear();
    let mut symbol = 0_usize;
    let mut space = 1_i32 << MAX_CODE_BITS;
    let mut previous_non_zero = 8_u8;
    let mut repeat = 0_usize;
    let mut repeat_len = 0_u8;

    while symbol < alphabet_size && space > 0 {
        let code = code_length_code.decode(reader)?;
        if code <= 15 {
            repeat = 0;
            if code != 0 {
                lists.push(symbol, code);
                previous_non_zero = code;
                space -= 1_i32 << (MAX_CODE_BITS - code);
            }
            symbol += 1;
            continue;
        }

        // Code 16 repeats the previous nonzero length and code 17 repeats
        // zero. A repeat right after one of the same code extends its run:
        // the run length so far, less two, scales by the extra bits' range.
        let extra_bits = code - 14;
        let len = if code == 16 { previous_non_zero } else { 0 };
        if len != repeat_len {
            repeat = 0;
            repeat_len = len;
        }
        let before = repeat;
        if repeat > 0 {
            repeat = (repeat - 2) << extra_bits;
        }
        repeat += reader.read_bits(extra_bits)? as usize + 3;
        let end = symbol + (repeat - before);
        if end > alphabet_size {
            return Err(BurliError::Format(
                "Brotli code length repeat exceeds alphabet",
            ));
        }
        if len != 0 {
            lists.push_run(symbol, end, len);
            space -= ((end - symbol) as i32) << (MAX_CODE_BITS - len);
        }
        symbol = end;
    }

    match space {
        0 => Ok(()),
        ..0 => Err(BurliError::Format("oversubscribed Brotli Huffman code")),
        _ => Err(BurliError::Format("incomplete Brotli Huffman code")),
    }
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
