//! Fixed command and distance codes for the one-pass q0 encoder.
//!
//! Each code comes with the bits of its header, so writing it costs a copy.
//! The flat code is the one
//! [`super::write_static_command_and_distance_prefix_codes`] writes, with
//! an 87-bit header. The tuned codes cost more header bits and fewer bits
//! per command. They come from Huffman codes for the command and distance
//! symbols of the one-pass encoder on 16 KiB and 64 KiB slices of the
//! Silesia and web corpora, with counts raised to a floor where that saves
//! header bits. Symbols that inputs of their size cannot produce get no
//! code.

use burli_core::bits::BitWriter;

/// Command cell by insert code group and copy code group, for commands that
/// carry a distance.
pub(super) const COMMAND_CELLS: [usize; 9] = [2, 3, 6, 4, 5, 8, 7, 9, 10];

/// Command and distance codes, each entry `len << 16 | bits` with the bits
/// reversed for LSB-first output, or 0 for no code. Command codes are
/// looked up by insert and copy length code.
#[derive(Clone, Debug)]
pub(super) struct CodeTables {
    /// By `insert_code * 32 + copy_code`, for commands with a distance.
    pub(super) explicit: [u32; 24 * 32],
    /// By `insert_code * 16 + copy_code`, for commands that reuse the last
    /// distance without a distance code.
    pub(super) implicit: [u32; 8 * 16],
    pub(super) distances: [u32; 64],
}

impl CodeTables {
    /// Arranges `commands` and `distances`, codes by symbol, for lookup by
    /// length code.
    pub(super) const fn new(commands: &[u32; 704], distances: [u32; 64]) -> Self {
        let mut tables = Self {
            explicit: [0; 24 * 32],
            implicit: [0; 8 * 16],
            distances,
        };
        let mut insert_code = 0;
        while insert_code < 24 {
            let mut copy_code = 0;
            while copy_code < 24 {
                tables.explicit[insert_code * 32 + copy_code] =
                    commands[explicit_symbol(insert_code, copy_code)];
                if insert_code < 8 && copy_code < 16 {
                    tables.implicit[insert_code * 16 + copy_code] =
                        commands[implicit_symbol(insert_code, copy_code)];
                }
                copy_code += 1;
            }
            insert_code += 1;
        }
        tables
    }
}

/// The command symbol for an insert and copy length code, with a distance.
pub(super) const fn explicit_symbol(insert_code: usize, copy_code: usize) -> usize {
    let cell = COMMAND_CELLS[(insert_code >> 3) * 3 + (copy_code >> 3)];
    cell * 64 + (insert_code & 7) * 8 + (copy_code & 7)
}

/// The command symbol for an insert length code below 8 and a copy length
/// code below 16, which reuses the last distance.
pub(super) const fn implicit_symbol(insert_code: usize, copy_code: usize) -> usize {
    (copy_code >> 3) * 64 + (insert_code & 7) * 8 + (copy_code & 7)
}

/// A fixed command and distance code with the bits of its header.
pub(super) struct FixedCode {
    pub(super) tables: CodeTables,
    header_bits: usize,
    header: &'static [u8],
}

impl FixedCode {
    /// Writes the command and distance prefix codes.
    pub(super) fn write_header(&self, writer: &mut BitWriter) {
        writer.write_bits_with(self.header_bits, |sink| {
            let mut left = self.header_bits;
            for chunk in self.header.chunks(7) {
                let mut word = [0; 8];
                word[..chunk.len()].copy_from_slice(chunk);
                let width = left.min(56);
                sink.write(width as u8, u64::from_le_bytes(word) & ((1 << width) - 1));
                left -= width;
            }
        });
    }
}

/// A fixed code as a type, so code generic over it finds the tables at a
/// constant address.
pub(super) trait Code {
    const CODE: &'static FixedCode;
}

/// The flat code, for inputs up to 4 KiB.
pub(super) struct Flat;
/// The tuned code for inputs up to 16 KiB.
pub(super) struct Short;
/// The tuned code for inputs up to 64 KiB.
pub(super) struct Long;

impl Code for Flat {
    const CODE: &'static FixedCode = &FLAT;
}

impl Code for Short {
    const CODE: &'static FixedCode = &SHORT;
}

impl Code for Long {
    const CODE: &'static FixedCode = &LONG;
}

/// 9 bits for commands below symbol 448, 11 bits from there, and 6 bits for
/// distances.
static FLAT: FixedCode = fixed_code(
    &{
        let mut lengths = [9; 704];
        let mut symbol = 448;
        while symbol < 704 {
            lengths[symbol] = 11;
            symbol += 1;
        }
        lengths
    },
    &[6; 64],
    87,
    &[3, 112, 48, 22, 68, 98, 146, 24, 224, 78, 27],
);

/// For inputs up to 16 KiB.
static SHORT: FixedCode = fixed_code(
    &SHORT_COMMAND_LENGTHS,
    &SHORT_DISTANCE_LENGTHS,
    1271,
    &SHORT_HEADER,
);

/// For inputs up to 64 KiB.
static LONG: FixedCode = fixed_code(
    &LONG_COMMAND_LENGTHS,
    &LONG_DISTANCE_LENGTHS,
    1344,
    &LONG_HEADER,
);

const SHORT_COMMAND_LENGTHS: [u8; 704] = [
    12, 0, 0, 12, 12, 12, 12, 12, 12, 0, 0, 9, 9, 7, 11, 11, 12, 0, 0, 10, 8, 11, 12, 12, 12, 0, 0,
    11, 12, 12, 12, 12, 12, 0, 0, 12, 12, 12, 12, 12, 12, 0, 0, 12, 12, 12, 12, 12, 12, 0, 0, 12,
    11, 12, 12, 12, 12, 0, 0, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 9, 10, 8, 9, 9,
    11, 10, 10, 11, 12, 9, 10, 11, 12, 12, 12, 12, 12, 12, 11, 12, 12, 12, 12, 12, 12, 12, 10, 12,
    12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12,
    12, 12, 12, 0, 0, 0, 3, 4, 4, 5, 6, 0, 0, 0, 4, 6, 6, 7, 8, 0, 0, 0, 6, 6, 8, 8, 9, 0, 0, 0, 6,
    7, 8, 9, 9, 0, 0, 0, 6, 7, 8, 9, 10, 0, 0, 0, 6, 8, 8, 9, 10, 0, 0, 0, 6, 7, 8, 9, 9, 0, 0, 0,
    7, 7, 8, 9, 10, 5, 6, 5, 6, 6, 8, 7, 8, 8, 9, 8, 8, 9, 10, 11, 10, 9, 9, 9, 10, 10, 10, 12, 12,
    9, 10, 9, 10, 10, 11, 12, 12, 9, 10, 10, 10, 11, 12, 12, 11, 9, 10, 10, 11, 11, 12, 12, 12, 9,
    10, 10, 11, 10, 12, 12, 12, 10, 11, 10, 11, 11, 12, 12, 12, 12, 0, 0, 7, 7, 8, 9, 10, 12, 0, 0,
    7, 8, 9, 10, 11, 12, 0, 0, 7, 8, 9, 10, 11, 12, 0, 0, 8, 9, 10, 11, 12, 12, 0, 0, 9, 10, 11,
    12, 12, 12, 0, 0, 9, 11, 11, 12, 12, 12, 0, 0, 9, 10, 11, 12, 12, 12, 0, 0, 11, 12, 12, 12, 12,
    10, 10, 10, 8, 11, 12, 12, 12, 10, 11, 10, 10, 12, 12, 12, 12, 11, 12, 11, 11, 12, 12, 12, 12,
    11, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12,
    12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 9, 10, 10, 10, 11, 12, 12, 12,
    11, 12, 11, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12,
    12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12,
    12, 12, 12, 12, 12, 12, 12, 12, 12, 0, 0, 12, 12, 12, 12, 12, 12, 0, 0, 12, 12, 12, 12, 12, 12,
    0, 0, 12, 12, 12, 12, 12, 12, 0, 0, 12, 12, 12, 12, 12, 12, 0, 0, 12, 12, 12, 12, 12, 12, 0, 0,
    12, 12, 12, 12, 12, 12, 0, 0, 12, 12, 12, 12, 12, 0, 0, 0, 0, 0, 0, 0, 0, 12, 12, 12, 12, 12,
    12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12,
    12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12,
    12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12,
    12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12,
    12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 0, 0, 0, 0, 0, 0,
    0, 0, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12,
    12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12,
    12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 0, 0, 0, 0, 0, 0, 0, 0,
];
const SHORT_DISTANCE_LENGTHS: [u8; 64] = [
    7, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 7, 7, 7, 7, 6, 6, 5, 5, 5, 5, 4, 5, 4, 4, 4, 4,
    4, 4, 4, 4, 4, 4, 4, 6, 7, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
];
const SHORT_HEADER: [u8; 159] = [
    254, 251, 255, 126, 170, 1, 58, 64, 62, 59, 128, 198, 73, 0, 167, 1, 29, 232, 64, 156, 2, 180,
    74, 13, 105, 201, 161, 156, 234, 116, 165, 180, 242, 177, 246, 62, 247, 99, 223, 251, 230, 227,
    222, 25, 126, 220, 55, 228, 199, 125, 67, 125, 220, 25, 234, 227, 190, 33, 63, 222, 27, 234,
    220, 115, 239, 188, 25, 206, 80, 22, 73, 73, 9, 69, 201, 9, 37, 57, 49, 37, 59, 9, 37, 43, 137,
    44, 59, 5, 120, 111, 168, 0, 111, 40, 7, 120, 67, 57, 192, 80, 78, 0, 202, 73, 0, 218, 73, 0,
    202, 73, 0, 167, 72, 26, 39, 145, 165, 20, 199, 78, 113, 74, 239, 148, 228, 36, 142, 211, 122,
    5, 58, 208, 129, 14, 116, 160, 3, 29, 168, 191, 165, 150, 246, 91, 75, 123, 23, 225, 0, 0, 219,
    246, 127, 119, 215, 85, 85, 85, 63,
];

pub(super) const LONG_COMMAND_LENGTHS: [u8; 704] = [
    13, 0, 0, 13, 13, 13, 13, 13, 13, 0, 0, 8, 9, 7, 10, 11, 13, 0, 0, 10, 8, 11, 12, 13, 13, 0, 0,
    11, 12, 13, 13, 13, 13, 0, 0, 12, 13, 13, 13, 13, 13, 0, 0, 12, 13, 12, 13, 13, 13, 0, 0, 12,
    12, 13, 13, 13, 13, 0, 0, 13, 13, 12, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 10, 11, 8, 9, 10,
    11, 11, 11, 11, 12, 9, 10, 11, 12, 12, 13, 13, 13, 13, 12, 13, 13, 13, 13, 13, 13, 13, 11, 13,
    13, 13, 13, 12, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13,
    13, 13, 13, 0, 0, 0, 3, 3, 4, 5, 5, 0, 0, 0, 4, 6, 6, 7, 8, 0, 0, 0, 6, 6, 7, 8, 9, 0, 0, 0, 6,
    7, 8, 9, 9, 0, 0, 0, 6, 7, 8, 9, 10, 0, 0, 0, 6, 8, 8, 10, 10, 0, 0, 0, 6, 7, 8, 9, 10, 0, 0,
    0, 7, 7, 8, 10, 11, 5, 6, 5, 6, 6, 7, 7, 7, 8, 9, 8, 8, 9, 10, 11, 10, 9, 10, 9, 10, 10, 10,
    12, 11, 9, 10, 10, 10, 10, 11, 12, 12, 10, 10, 10, 10, 11, 12, 13, 11, 10, 11, 11, 11, 11, 13,
    13, 12, 9, 10, 10, 11, 11, 12, 13, 13, 10, 11, 11, 12, 12, 13, 13, 13, 13, 0, 0, 7, 8, 8, 10,
    10, 13, 0, 0, 8, 9, 10, 11, 12, 13, 0, 0, 8, 9, 10, 11, 12, 13, 0, 0, 9, 10, 11, 12, 13, 13, 0,
    0, 9, 11, 12, 13, 13, 13, 0, 0, 10, 12, 13, 13, 13, 13, 0, 0, 10, 12, 13, 13, 13, 13, 0, 0, 13,
    13, 13, 13, 13, 10, 11, 11, 7, 12, 13, 13, 13, 11, 11, 11, 11, 13, 13, 13, 13, 12, 13, 12, 12,
    13, 13, 13, 13, 12, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13,
    13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 9, 10, 10, 10,
    11, 12, 13, 13, 12, 13, 13, 12, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13,
    13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13,
    13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 0, 0, 13, 13, 13, 13, 13, 13, 0, 0, 13, 13,
    13, 13, 13, 13, 0, 0, 13, 13, 13, 13, 13, 13, 0, 0, 13, 13, 13, 13, 13, 13, 0, 0, 13, 13, 13,
    13, 13, 13, 0, 0, 13, 13, 13, 13, 13, 13, 0, 0, 13, 13, 13, 13, 13, 13, 0, 0, 13, 13, 13, 13,
    13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13,
    13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13,
    13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13,
    13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13,
    13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13,
    13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13,
    13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13,
    13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13,
    13,
];
pub(super) const LONG_DISTANCE_LENGTHS: [u8; 64] = [
    9, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 8, 10, 8, 8, 6, 7, 6, 6, 5, 6, 5, 5, 5, 5, 4,
    4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 5, 5, 7, 10, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0,
];
const LONG_HEADER: [u8; 168] = [
    254, 251, 255, 126, 153, 26, 200, 64, 245, 227, 24, 96, 141, 108, 96, 228, 0, 200, 9, 144, 101,
    27, 144, 28, 0, 91, 14, 145, 83, 205, 9, 106, 142, 228, 32, 231, 113, 144, 115, 254, 88, 107,
    159, 243, 177, 239, 125, 245, 113, 239, 171, 254, 184, 175, 186, 63, 238, 171, 230, 199, 173,
    34, 63, 238, 171, 230, 199, 123, 197, 57, 247, 220, 251, 222, 171, 174, 106, 14, 155, 77, 82,
    211, 12, 35, 49, 140, 60, 156, 96, 171, 201, 25, 217, 156, 145, 28, 128, 87, 69, 26, 168, 230,
    200, 64, 53, 71, 6, 154, 35, 27, 232, 145, 109, 128, 114, 0, 40, 7, 32, 114, 230, 201, 246, 4,
    7, 89, 114, 144, 67, 206, 77, 114, 100, 203, 150, 83, 78, 64, 6, 50, 144, 129, 12, 100, 32, 3,
    25, 72, 49, 199, 55, 145, 247, 251, 0, 0, 0, 192, 110, 235, 107, 211, 36, 81, 85, 85, 147, 119,
];

/// The fixed code for `command_lengths` and `distance_lengths`.
const fn fixed_code(
    command_lengths: &[u8; 704],
    distance_lengths: &[u8; 64],
    header_bits: usize,
    header: &'static [u8],
) -> FixedCode {
    FixedCode {
        tables: CodeTables::new(
            &canonical_codes(command_lengths),
            canonical_codes(distance_lengths),
        ),
        header_bits,
        header,
    }
}

/// Canonical prefix codes for `lengths`, as the decoder assigns them.
const fn canonical_codes<const N: usize>(lengths: &[u8; N]) -> [u32; N] {
    let mut counts = [0_u32; 16];
    let mut symbol = 0;
    while symbol < N {
        counts[lengths[symbol] as usize] += 1;
        symbol += 1;
    }
    counts[0] = 0;
    let mut next = [0_u32; 16];
    let mut code = 0;
    let mut bits = 1;
    while bits < 16 {
        code = (code + counts[bits - 1]) << 1;
        next[bits] = code;
        bits += 1;
    }
    let mut codes = [0; N];
    let mut symbol = 0;
    while symbol < N {
        let len = lengths[symbol] as u32;
        if len != 0 {
            let code = next[len as usize];
            next[len as usize] += 1;
            codes[symbol] = (len << 16) | (code.reverse_bits() >> (32 - len));
        }
        symbol += 1;
    }
    codes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encode::DenseSymbolCode;

    fn header(code: &FixedCode) -> (usize, alloc::vec::Vec<u8>) {
        let mut writer = BitWriter::new();
        code.write_header(&mut writer);
        (writer.written_bits(), writer.into_bytes())
    }

    #[test]
    fn headers_describe_codes() {
        let mut flat = BitWriter::new();
        super::super::write_static_command_and_distance_prefix_codes(&mut flat);
        assert_eq!(header(&FLAT), (flat.written_bits(), flat.into_bytes()));

        for (code, commands, distances) in [
            (&SHORT, &SHORT_COMMAND_LENGTHS, &SHORT_DISTANCE_LENGTHS),
            (&LONG, &LONG_COMMAND_LENGTHS, &LONG_DISTANCE_LENGTHS),
        ] {
            let mut general = BitWriter::new();
            super::super::write_complex_prefix_code_lengths(&mut general, commands).unwrap();
            super::super::write_complex_prefix_code_lengths(&mut general, distances).unwrap();
            assert_eq!(header(code), (general.written_bits(), general.into_bytes()));
        }
    }

    #[test]
    fn flat_codes_match_static_codes() {
        let pack = |code: DenseSymbolCode| (u32::from(code.len) << 16) | u32::from(code.bits);
        for insert_code in 0..24 {
            for copy_code in 0..24 {
                let symbol =
                    super::super::command_symbol_for_insert_copy(insert_code, copy_code, false)
                        .unwrap();
                let general = super::super::static_command_code(symbol).unwrap();
                assert_eq!(
                    FLAT.tables.explicit[insert_code * 32 + copy_code],
                    pack(general)
                );
                if insert_code < 8 && copy_code < 16 {
                    let symbol =
                        super::super::command_symbol_for_insert_copy(insert_code, copy_code, true)
                            .unwrap();
                    let general = super::super::static_command_code(symbol).unwrap();
                    assert_eq!(
                        FLAT.tables.implicit[insert_code * 16 + copy_code],
                        pack(general)
                    );
                }
            }
        }
        for (symbol, &code) in FLAT.tables.distances.iter().enumerate() {
            let general = super::super::static_distance_code(symbol as u16).unwrap();
            assert_eq!(code, pack(general), "symbol {symbol}");
        }
    }
}
