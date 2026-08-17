#[cfg(feature = "alloc")]
use alloc::vec::Vec;

use crate::{BurliError, Result};

pub const MAX_BITS_PER_OP: u8 = 56;

#[derive(Clone, Debug)]
pub struct BitReader<'a> {
    input: &'a [u8],
    bit_pos: usize,
}

impl<'a> BitReader<'a> {
    pub const fn new(input: &'a [u8]) -> Self {
        Self { input, bit_pos: 0 }
    }

    pub fn with_bit_pos(input: &'a [u8], bit_pos: usize) -> Result<Self> {
        if bit_pos > input.len() * 8 {
            return Err(BurliError::Format("Brotli bit position exceeds input"));
        }
        Ok(Self { input, bit_pos })
    }

    pub const fn consumed_bits(&self) -> usize {
        self.bit_pos
    }

    pub const fn remaining_bits(&self) -> usize {
        (self.input.len() * 8).saturating_sub(self.bit_pos)
    }

    #[inline(always)]
    pub fn has_bits(&self, width: u8) -> bool {
        let total_bits = self.input.len() * 8;
        self.bit_pos <= total_bits && total_bits - self.bit_pos >= usize::from(width)
    }

    pub const fn is_byte_aligned(&self) -> bool {
        self.bit_pos.is_multiple_of(8)
    }

    #[inline(always)]
    pub fn read_bit(&mut self) -> Result<bool> {
        Ok(self.read_bits(1)? != 0)
    }

    #[inline(always)]
    pub fn read_bits(&mut self, width: u8) -> Result<u64> {
        Self::validate_bit_width(width, "bit read width exceeds 56 bits")?;
        let width = usize::from(width);
        if self.remaining_bits() < width {
            return Err(BurliError::Format("unexpected end of Brotli input"));
        }
        if width == 0 {
            return Ok(0);
        }

        let value = self.peek_bits_unchecked(width);
        self.bit_pos += width;
        Ok(value)
    }

    #[inline(always)]
    pub fn peek_bits(&self, width: u8) -> Result<u64> {
        Self::validate_bit_width(width, "bit read width exceeds 56 bits")?;
        let width = usize::from(width);
        if self.remaining_bits() < width {
            return Err(BurliError::Format("unexpected end of Brotli input"));
        }
        if width == 0 {
            return Ok(0);
        }

        Ok(self.peek_bits_unchecked(width))
    }

    #[inline(always)]
    fn peek_bits_unchecked(&self, width: usize) -> u64 {
        debug_assert!(width <= usize::from(MAX_BITS_PER_OP));
        debug_assert!(self.remaining_bits() >= width);
        debug_assert!(width != 0);
        self.peek_bits_unchecked_with_mask(width, (1_u64 << width) - 1)
    }

    #[inline(always)]
    fn peek_bits_unchecked_with_mask(&self, width: usize, mask: u64) -> u64 {
        debug_assert!(width <= usize::from(MAX_BITS_PER_OP));
        debug_assert!(self.remaining_bits() >= width);
        debug_assert!(width != 0);

        let byte_pos = self.bit_pos / 8;
        let bit_offset = self.bit_pos % 8;
        let mut value = if let Some(bytes) = self.input[byte_pos..].first_chunk::<8>() {
            u64::from_le_bytes(*bytes)
        } else {
            self.peek_bits_tail(byte_pos, bit_offset, width)
        };

        value >>= bit_offset;
        value & mask
    }

    #[cold]
    #[inline(never)]
    fn peek_bits_tail(&self, byte_pos: usize, bit_offset: usize, width: usize) -> u64 {
        let byte_count = (bit_offset + width).div_ceil(8);
        let bytes = &self.input[byte_pos..byte_pos + byte_count];
        let mut value = 0_u64;
        for (index, &byte) in bytes.iter().enumerate() {
            value |= u64::from(byte) << (index * 8);
        }
        value
    }

    #[inline(always)]
    pub fn drop_bits(&mut self, width: u8) -> Result<()> {
        Self::validate_bit_width(width, "bit drop width exceeds 56 bits")?;
        let width = usize::from(width);
        if self.remaining_bits() < width {
            return Err(BurliError::Format("unexpected end of Brotli input"));
        }

        self.bit_pos += width;
        Ok(())
    }

    #[doc(hidden)]
    #[inline(always)]
    pub fn peek_bits_trusted(&self, width: u8) -> u64 {
        let width = usize::from(width);
        self.peek_bits_unchecked(width)
    }

    #[doc(hidden)]
    #[inline(always)]
    pub fn peek_bits_trusted_with_mask(&self, width: u8, mask: u64) -> u64 {
        self.peek_bits_unchecked_with_mask(usize::from(width), mask)
    }

    #[doc(hidden)]
    #[inline(always)]
    pub fn drop_bits_trusted(&mut self, width: u8) {
        debug_assert!(width <= MAX_BITS_PER_OP);
        debug_assert!(self.remaining_bits() >= usize::from(width));
        self.bit_pos += usize::from(width);
    }

    #[inline(always)]
    fn validate_bit_width(width: u8, message: &'static str) -> Result<()> {
        if width > MAX_BITS_PER_OP {
            return Err(BurliError::Format(message));
        }
        Ok(())
    }

    pub fn align_to_byte(&mut self) {
        self.bit_pos = (self.bit_pos + 7) & !7;
    }

    pub fn read_zero_padding_to_byte(&mut self) -> Result<()> {
        let padding = (8 - (self.bit_pos % 8)) % 8;
        if padding == 0 {
            return Ok(());
        }

        if self.read_bits(padding as u8)? != 0 {
            return Err(BurliError::Format("non-zero Brotli byte padding"));
        }

        Ok(())
    }

    pub fn read_aligned_bytes(&mut self, len: usize) -> Result<&'a [u8]> {
        if !self.is_byte_aligned() {
            return Err(BurliError::Format("Brotli reader is not byte aligned"));
        }

        let start = self.bit_pos / 8;
        let end = start
            .checked_add(len)
            .ok_or(BurliError::Format("Brotli input byte range overflow"))?;
        let bytes = self
            .input
            .get(start..end)
            .ok_or(BurliError::Format("unexpected end of Brotli input"))?;
        self.bit_pos += len * 8;
        Ok(bytes)
    }

    pub fn remaining_bits_are_zero(&self) -> bool {
        if self.remaining_bits() == 0 {
            return true;
        }
        let start_byte = self.bit_pos / 8;
        let bit_offset = self.bit_pos % 8;
        if bit_offset != 0 {
            let mask = !((1u8 << bit_offset) - 1);
            if self.input[start_byte] & mask != 0 {
                return false;
            }
            return self.input[start_byte + 1..].iter().all(|&b| b == 0);
        }
        self.input[start_byte..].iter().all(|&b| b == 0)
    }
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
