#[cfg(feature = "alloc")]
use alloc::vec::Vec;

use crate::{BurliError, Result};

pub const MAX_BITS_PER_OP: u8 = 56;

/// LSB-first bit reader with a 64-bit buffer.
///
/// `buffer` holds the next `bit_count` input bits. Bits above `bit_count` are
/// either the input bits that follow or zero, never other data. Refills load
/// whole words, so a refill leaves at least 56 buffered bits or buffers every
/// remaining input bit.
#[derive(Clone, Debug)]
pub struct BitReader<'a> {
    input: &'a [u8],
    /// Next input byte to load into `buffer`.
    byte_pos: usize,
    buffer: u64,
    bit_count: u32,
}

impl<'a> BitReader<'a> {
    pub const fn new(input: &'a [u8]) -> Self {
        Self {
            input,
            byte_pos: 0,
            buffer: 0,
            bit_count: 0,
        }
    }

    pub fn with_bit_pos(input: &'a [u8], bit_pos: usize) -> Result<Self> {
        if bit_pos > input.len() * 8 {
            return Err(BurliError::Format("Brotli bit position exceeds input"));
        }
        let mut reader = Self::new(input);
        reader.byte_pos = bit_pos / 8;
        let bit_offset = (bit_pos % 8) as u32;
        if bit_offset != 0 {
            reader.buffer = u64::from(input[reader.byte_pos] >> bit_offset);
            reader.bit_count = 8 - bit_offset;
            reader.byte_pos += 1;
        }
        Ok(reader)
    }

    pub const fn consumed_bits(&self) -> usize {
        self.byte_pos * 8 - self.bit_count as usize
    }

    pub const fn remaining_bits(&self) -> usize {
        (self.input.len() - self.byte_pos) * 8 + self.bit_count as usize
    }

    #[inline(always)]
    pub fn has_bits(&self, width: u8) -> bool {
        self.remaining_bits() >= usize::from(width)
    }

    pub const fn is_byte_aligned(&self) -> bool {
        self.bit_count.is_multiple_of(8)
    }

    #[inline(always)]
    pub fn read_bit(&mut self) -> Result<bool> {
        Ok(self.read_bits(1)? != 0)
    }

    #[inline(always)]
    pub fn read_bits(&mut self, width: u8) -> Result<u64> {
        Self::validate_bit_width(width, "bit read width exceeds 56 bits")?;
        self.fill(width);
        if self.bit_count < u32::from(width) {
            return Err(BurliError::Format("unexpected end of Brotli input"));
        }
        let value = self.buffer & low_mask(width);
        self.consume(width);
        Ok(value)
    }

    #[inline(always)]
    pub fn peek_bits(&self, width: u8) -> Result<u64> {
        Self::validate_bit_width(width, "bit read width exceeds 56 bits")?;
        if self.remaining_bits() < usize::from(width) {
            return Err(BurliError::Format("unexpected end of Brotli input"));
        }
        Ok(self.peek_bits_unchecked(width))
    }

    /// Returns the next `width` bits without refilling. The caller checks
    /// that the input holds them.
    #[inline(always)]
    fn peek_bits_unchecked(&self, width: u8) -> u64 {
        debug_assert!(width <= MAX_BITS_PER_OP);
        debug_assert!(self.remaining_bits() >= usize::from(width));
        if self.bit_count >= u32::from(width) {
            return self.buffer & low_mask(width);
        }
        peek_bits_unbuffered(
            &self.input[self.byte_pos..],
            self.buffer,
            self.bit_count,
            width,
        )
    }

    #[inline(always)]
    pub fn drop_bits(&mut self, width: u8) -> Result<()> {
        Self::validate_bit_width(width, "bit drop width exceeds 56 bits")?;
        self.fill(width);
        if self.bit_count < u32::from(width) {
            return Err(BurliError::Format("unexpected end of Brotli input"));
        }
        self.consume(width);
        Ok(())
    }

    #[doc(hidden)]
    #[inline(always)]
    pub fn peek_bits_trusted(&self, width: u8) -> u64 {
        self.peek_bits_unchecked(width)
    }

    #[doc(hidden)]
    #[inline(always)]
    pub fn peek_bits_trusted_with_mask(&self, width: u8, mask: u64) -> u64 {
        self.peek_bits_unchecked(width) & mask
    }

    /// Refills when fewer than `width` bits are buffered. Afterwards at least
    /// `min(width, remaining_bits)` bits are buffered, for `width <= 56`.
    #[doc(hidden)]
    #[inline(always)]
    pub fn fill(&mut self, width: u8) {
        debug_assert!(width <= MAX_BITS_PER_OP);
        if self.bit_count < u32::from(width) {
            self.refill();
        }
    }

    /// Loads input into the buffer. Afterwards at least
    /// `min(56, remaining_bits)` bits are buffered.
    #[doc(hidden)]
    #[inline(always)]
    pub fn refill(&mut self) {
        match self
            .input
            .get(self.byte_pos..)
            .and_then(<[u8]>::first_chunk::<8>)
        {
            Some(bytes) => {
                // Bits of the word past the whole bytes taken are the input
                // bits that follow, which keeps the buffer invariant.
                self.buffer |= u64::from_le_bytes(*bytes) << self.bit_count;
                let taken = (63 - self.bit_count) / 8;
                self.byte_pos += taken as usize;
                self.bit_count += taken * 8;
            }
            None => {
                let (taken, buffer, bit_count) =
                    refill_tail(&self.input[self.byte_pos..], self.buffer, self.bit_count);
                self.byte_pos += taken;
                self.buffer = buffer;
                self.bit_count = bit_count;
            }
        }
    }

    /// Returns the buffered bits. Only the low [`Self::buffered_bits`] bits
    /// are guaranteed. Above them are the following input bits or zeros, and
    /// zeros past the input end.
    #[doc(hidden)]
    #[inline(always)]
    pub const fn buffer(&self) -> u64 {
        self.buffer
    }

    #[doc(hidden)]
    #[inline(always)]
    pub const fn buffered_bits(&self) -> u32 {
        self.bit_count
    }

    /// Returns upcoming bits with zeros past the input end. At least 56 bits
    /// are valid when the input has them.
    #[doc(hidden)]
    #[inline(always)]
    pub fn peek_bits_padded(&mut self) -> u64 {
        self.fill(MAX_BITS_PER_OP);
        self.buffer
    }

    #[doc(hidden)]
    #[inline(always)]
    pub fn drop_bits_trusted(&mut self, width: u8) {
        debug_assert!(width <= MAX_BITS_PER_OP);
        debug_assert!(self.bit_count >= u32::from(width));
        self.consume(width);
    }

    #[inline(always)]
    fn consume(&mut self, width: u8) {
        debug_assert!(self.bit_count >= u32::from(width));
        self.buffer >>= width;
        self.bit_count -= u32::from(width);
    }

    #[inline(always)]
    fn validate_bit_width(width: u8, message: &'static str) -> Result<()> {
        if width > MAX_BITS_PER_OP {
            return Err(BurliError::Format(message));
        }
        Ok(())
    }

    pub fn align_to_byte(&mut self) {
        let padding = self.bit_count % 8;
        self.consume(padding as u8);
    }

    pub fn read_zero_padding_to_byte(&mut self) -> Result<()> {
        let padding = (self.bit_count % 8) as u8;
        if padding == 0 {
            return Ok(());
        }

        if self.read_bits(padding)? != 0 {
            return Err(BurliError::Format("non-zero Brotli byte padding"));
        }

        Ok(())
    }

    pub fn read_aligned_bytes(&mut self, len: usize) -> Result<&'a [u8]> {
        if !self.is_byte_aligned() {
            return Err(BurliError::Format("Brotli reader is not byte aligned"));
        }

        let start = self.consumed_bits() / 8;
        let end = start
            .checked_add(len)
            .ok_or(BurliError::Format("Brotli input byte range overflow"))?;
        let bytes = self
            .input
            .get(start..end)
            .ok_or(BurliError::Format("unexpected end of Brotli input"))?;
        self.byte_pos = end;
        self.buffer = 0;
        self.bit_count = 0;
        Ok(bytes)
    }

    pub fn remaining_bits_are_zero(&self) -> bool {
        self.buffer & low_mask(self.bit_count as u8) == 0
            && self.input[self.byte_pos..].iter().all(|&b| b == 0)
    }
}

// The cold helpers take reader state by value so that a reader held in a
// local never has its address taken and can stay in registers.

#[cold]
#[inline(never)]
fn peek_bits_unbuffered(rest: &[u8], buffer: u64, bit_count: u32, width: u8) -> u64 {
    let mut value = buffer & low_mask(bit_count as u8);
    let mut shift = bit_count;
    for &byte in rest {
        if shift >= u32::from(width) {
            break;
        }
        value |= u64::from(byte) << shift;
        shift += 8;
    }
    value & low_mask(width)
}

/// Loads bytes one at a time until at least 56 bits are buffered or `rest`
/// runs out. Returns the byte count taken and the new buffer state.
#[cold]
#[inline(never)]
fn refill_tail(rest: &[u8], mut buffer: u64, mut bit_count: u32) -> (usize, u64, u32) {
    let mut taken = 0;
    while bit_count <= 55 {
        let Some(&byte) = rest.get(taken) else {
            break;
        };
        buffer |= u64::from(byte) << bit_count;
        taken += 1;
        bit_count += 8;
    }
    (taken, buffer, bit_count)
}

/// Mask of the low `width` bits, for `width <= 63`.
#[inline(always)]
const fn low_mask(width: u8) -> u64 {
    (1_u64 << width) - 1
}

#[cfg(feature = "alloc")]
#[derive(Clone, Debug, Default)]
pub struct BitWriter {
    output: Vec<u8>,
    bit_buffer: u64,
    bit_count: u8,
    bit_len: usize,
}

#[cfg(feature = "alloc")]
impl BitWriter {
    pub const fn new() -> Self {
        Self {
            output: Vec::new(),
            bit_buffer: 0,
            bit_count: 0,
            bit_len: 0,
        }
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            output: Vec::with_capacity(capacity),
            bit_buffer: 0,
            bit_count: 0,
            bit_len: 0,
        }
    }

    pub fn clear(&mut self) {
        self.output.clear();
        self.bit_buffer = 0;
        self.bit_count = 0;
        self.bit_len = 0;
    }

    pub fn reserve(&mut self, additional: usize) {
        self.output.reserve(additional);
    }

    pub fn written_bits(&self) -> usize {
        self.bit_len
    }

    #[inline(always)]
    pub fn write_bits(&mut self, width: u8, value: u64) -> Result<()> {
        if width > MAX_BITS_PER_OP {
            return Err(BurliError::Format("bit write width exceeds 56 bits"));
        }

        let width = usize::from(width);
        if width == 0 {
            return Ok(());
        }

        self.bit_len
            .checked_add(width)
            .ok_or(BurliError::Format("Brotli output bit length overflow"))?;
        self.write_bits_trusted(width as u8, value);
        Ok(())
    }

    #[doc(hidden)]
    #[inline(always)]
    pub fn write_bits_trusted(&mut self, width: u8, value: u64) {
        debug_assert!(width <= MAX_BITS_PER_OP);
        let width = usize::from(width);
        if width == 0 {
            return;
        }
        debug_assert!(self.bit_len.checked_add(width).is_some());

        self.bit_len = self.bit_len.wrapping_add(width);

        let mask = (1_u64 << width) - 1;
        self.bit_buffer |= (value & mask) << self.bit_count;
        self.bit_count += width as u8;
        self.flush_full_bytes();
    }

    #[doc(hidden)]
    #[inline(always)]
    pub fn write_bits_trusted_fits(&mut self, width: u8, value: u64) {
        debug_assert!(width <= MAX_BITS_PER_OP);
        debug_assert!(width == 0 || value < (1_u64 << width));
        let width = usize::from(width);
        if width == 0 {
            return;
        }
        debug_assert!(self.bit_len.checked_add(width).is_some());

        self.bit_len = self.bit_len.wrapping_add(width);
        self.bit_buffer |= value << self.bit_count;
        self.bit_count += width as u8;
        self.flush_full_bytes();
    }

    #[doc(hidden)]
    #[inline(always)]
    pub fn write_bits_trusted_nonzero_fits(&mut self, width: u8, value: u64) {
        debug_assert!(width != 0);
        debug_assert!(width <= MAX_BITS_PER_OP);
        debug_assert!(value < (1_u64 << width));
        debug_assert!(self.bit_len.checked_add(usize::from(width)).is_some());

        self.bit_len = self.bit_len.wrapping_add(usize::from(width));
        self.bit_buffer |= value << self.bit_count;
        self.bit_count += width;
        self.flush_full_bytes();
    }

    #[inline(always)]
    fn flush_full_bytes(&mut self) {
        let byte_count = self.bit_count / 8;
        if byte_count == 0 {
            return;
        }
        let bytes = self.bit_buffer.to_le_bytes();
        match byte_count {
            1 => self.output.push(bytes[0]),
            _ => self
                .output
                .extend_from_slice(&bytes[..usize::from(byte_count)]),
        }
        self.bit_buffer >>= byte_count * 8;
        self.bit_count -= byte_count * 8;
    }

    pub fn align_to_byte(&mut self) -> Result<()> {
        let padding = (8 - (self.bit_len % 8)) % 8;
        self.write_bits(padding as u8, 0)
    }

    pub fn write_aligned_bytes(&mut self, bytes: &[u8]) -> Result<()> {
        self.align_to_byte()?;
        debug_assert_eq!(self.bit_count, 0);
        self.output.extend_from_slice(bytes);
        self.bit_len = self
            .bit_len
            .checked_add(bytes.len() * 8)
            .ok_or(BurliError::Format("Brotli output bit length overflow"))?;
        Ok(())
    }

    pub fn take_full_bytes(&mut self) -> Vec<u8> {
        self.bit_len = usize::from(self.bit_count);
        core::mem::take(&mut self.output)
    }

    pub fn into_bytes(mut self) -> Vec<u8> {
        if self.bit_count != 0 {
            self.output.push(self.bit_buffer as u8);
        }
        self.output
    }

    pub fn finish_into(&mut self, output: &mut Vec<u8>) -> usize {
        if self.bit_count != 0 {
            self.output.push(self.bit_buffer as u8);
        }
        let before = output.len();
        output.extend_from_slice(&self.output);
        self.clear();
        output.len() - before
    }

    pub fn finished_len(&self) -> usize {
        self.output.len() + usize::from(self.bit_count != 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_lsb_first_bits() {
        let mut reader = BitReader::new(&[0b1010_1100, 0b0000_0011]);

        assert_eq!(reader.read_bits(3).unwrap(), 0b100);
        assert_eq!(reader.read_bits(5).unwrap(), 0b10101);
        assert_eq!(reader.read_bits(2).unwrap(), 0b11);
        assert_eq!(reader.consumed_bits(), 10);
        assert!(reader.remaining_bits_are_zero());
    }

    #[cfg(feature = "alloc")]
    #[test]
    fn writer_round_trips_bits() {
        let mut writer = BitWriter::new();
        writer.write_bits(3, 0b101).unwrap();
        writer.write_bits(5, 0b11_001).unwrap();
        writer.write_bits(8, 0xa6).unwrap();

        let encoded = writer.into_bytes();
        let mut reader = BitReader::new(&encoded);
        assert_eq!(reader.read_bits(3).unwrap(), 0b101);
        assert_eq!(reader.read_bits(5).unwrap(), 0b11_001);
        assert_eq!(reader.read_bits(8).unwrap(), 0xa6);
    }

    #[cfg(feature = "alloc")]
    #[test]
    fn aligned_byte_write_pads_with_zeroes() {
        let mut writer = BitWriter::new();
        writer.write_bits(3, 0b111).unwrap();
        writer.write_aligned_bytes(b"ok").unwrap();

        let encoded = writer.into_bytes();
        let mut reader = BitReader::new(&encoded);
        assert_eq!(reader.read_bits(3).unwrap(), 0b111);
        reader.read_zero_padding_to_byte().unwrap();
        assert_eq!(reader.read_aligned_bytes(2).unwrap(), b"ok");
    }

    #[cfg(feature = "alloc")]
    #[test]
    fn take_full_bytes_keeps_partial_tail() {
        let mut writer = BitWriter::new();
        writer.write_bits(12, 0xabc).unwrap();

        let full = writer.take_full_bytes();
        writer.write_bits(4, 0x0d).unwrap();
        let rest = writer.into_bytes();

        assert_eq!(full, [0xbc]);
        assert_eq!(rest, [0xda]);
    }

    #[cfg(feature = "alloc")]
    #[test]
    fn trusted_writer_matches_checked_writer() {
        let writes = [
            (0, 0),
            (1, 1),
            (7, 0x7a),
            (8, 0xa5),
            (15, 0x5a5a),
            (24, 0x00ad_beef),
            (56, 0x00c0_ffee_d15c_a11e),
            (3, 0b101),
        ];
        let mut checked = BitWriter::new();
        let mut trusted = BitWriter::new();

        for (width, value) in writes {
            checked.write_bits(width, value).unwrap();
            trusted.write_bits_trusted(width, value);
            assert_eq!(trusted.written_bits(), checked.written_bits());
        }

        assert_eq!(trusted.into_bytes(), checked.into_bytes());
    }

    /// Reads bit by bit, as the buffered reader must behave.
    fn naive_bits(input: &[u8], start: usize, width: usize) -> Option<u64> {
        if start + width > input.len() * 8 {
            return None;
        }
        let mut value = 0_u64;
        for offset in 0..width {
            let bit = start + offset;
            value |= u64::from((input[bit / 8] >> (bit % 8)) & 1) << offset;
        }
        Some(value)
    }

    #[cfg(feature = "alloc")]
    #[test]
    fn buffered_reader_matches_naive_reader() {
        let mut state = 0x9e37_79b9_7f4a_7c15_u64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        for len in 0..40 {
            let input: Vec<u8> = (0..len).map(|_| next() as u8).collect();
            for start in 0..=len * 8 {
                let mut reader = BitReader::with_bit_pos(&input, start).unwrap();
                let mut pos = start;
                for _ in 0..24 {
                    assert_eq!(reader.consumed_bits(), pos);
                    assert_eq!(reader.remaining_bits(), len * 8 - pos);
                    let width = (next() % 57) as u8;
                    let expected = naive_bits(&input, pos, usize::from(width));
                    assert_eq!(reader.peek_bits(width).ok(), expected);
                    match next() % 4 {
                        0 => {
                            let aligned = pos.div_ceil(8) * 8;
                            if aligned > len * 8 {
                                break;
                            }
                            let padding_zero = naive_bits(&input, pos, aligned - pos) == Some(0);
                            assert_eq!(reader.read_zero_padding_to_byte().is_ok(), padding_zero);
                            if !padding_zero {
                                break;
                            }
                            pos = aligned;
                            let count = (next() % 4) as usize;
                            let bytes = reader.read_aligned_bytes(count);
                            if pos / 8 + count > len {
                                assert!(bytes.is_err());
                                break;
                            }
                            assert_eq!(bytes.unwrap(), &input[pos / 8..pos / 8 + count]);
                            pos += count * 8;
                        }
                        1 => {
                            reader.fill(width);
                            if reader.buffered_bits() < u32::from(width) {
                                assert_eq!(
                                    reader.remaining_bits(),
                                    reader.buffered_bits() as usize
                                );
                            }
                            assert_eq!(reader.drop_bits(width).is_ok(), expected.is_some());
                            if expected.is_none() {
                                break;
                            }
                            pos += usize::from(width);
                        }
                        _ => {
                            assert_eq!(reader.read_bits(width).ok(), expected);
                            if expected.is_none() {
                                break;
                            }
                            pos += usize::from(width);
                        }
                    }
                    let rest_zero = (pos..len * 8).all(|bit| naive_bits(&input, bit, 1) == Some(0));
                    assert_eq!(reader.remaining_bits_are_zero(), rest_zero);
                }
            }
        }
    }

    #[test]
    fn rejects_non_zero_byte_padding() {
        let mut reader = BitReader::new(&[0b0000_1000]);

        assert_eq!(reader.read_bits(3).unwrap(), 0);
        assert!(matches!(
            reader.read_zero_padding_to_byte(),
            Err(BurliError::Format("non-zero Brotli byte padding"))
        ));
    }
}

#[cfg(kani)]
mod verification {
    use super::*;

    #[kani::proof]
    #[kani::unwind(17)]
    fn bit_reader_matches_manual_lsb_extract() {
        let bytes = [kani::any::<u8>(), kani::any::<u8>()];
        let start = kani::any::<u8>();
        let width = kani::any::<u8>();
        kani::assume(start <= 15);
        kani::assume(width <= 16 - start);

        let mut expected = 0_u64;
        for offset in 0..width {
            let absolute = usize::from(start + offset);
            let bit = (bytes[absolute / 8] >> (absolute % 8)) & 1;
            expected |= u64::from(bit) << offset;
        }

        let mut reader = BitReader::new(&bytes);
        let _ = reader.read_bits(start).unwrap();
        let actual = reader.read_bits(width).unwrap();

        assert_eq!(actual, expected);
    }

    #[kani::proof]
    #[kani::unwind(17)]
    fn peek_bits_matches_read_bits_without_advancing() {
        let bytes = [kani::any::<u8>(), kani::any::<u8>()];
        let start = kani::any::<u8>();
        let width = kani::any::<u8>();
        kani::assume(start <= 15);
        kani::assume(width <= 16 - start);

        let mut reader = BitReader::new(&bytes);
        let _ = reader.drop_bits(start).unwrap();
        let before = reader.consumed_bits();
        let peeked = reader.peek_bits(width).unwrap();

        assert_eq!(reader.consumed_bits(), before);

        let read = reader.read_bits(width).unwrap();
        assert_eq!(peeked, read);
        assert_eq!(reader.consumed_bits(), before + usize::from(width));
    }
}
