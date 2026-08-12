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

    pub const fn consumed_bits(&self) -> usize {
        self.bit_pos
    }

    pub const fn remaining_bits(&self) -> usize {
        (self.input.len() * 8).saturating_sub(self.bit_pos)
    }

    pub const fn is_byte_aligned(&self) -> bool {
        self.bit_pos.is_multiple_of(8)
    }

    pub fn read_bit(&mut self) -> Result<bool> {
        Ok(self.read_bits(1)? != 0)
    }

    pub fn read_bits(&mut self, width: u8) -> Result<u64> {
        let value = self.peek_bits(width)?;
        self.drop_bits(width)?;
        Ok(value)
    }

    pub fn peek_bits(&self, width: u8) -> Result<u64> {
        if width > MAX_BITS_PER_OP {
            return Err(BurliError::Format("bit read width exceeds 56 bits"));
        }

        let width = usize::from(width);
        if self.remaining_bits() < width {
            return Err(BurliError::Format("unexpected end of Brotli input"));
        }

        let mut value = 0_u64;
        for offset in 0..width {
            let absolute = self.bit_pos + offset;
            let bit = (self.input[absolute / 8] >> (absolute % 8)) & 1;
            value |= u64::from(bit) << offset;
        }

        Ok(value)
    }

    pub fn drop_bits(&mut self, width: u8) -> Result<()> {
        if width > MAX_BITS_PER_OP {
            return Err(BurliError::Format("bit drop width exceeds 56 bits"));
        }

        let width = usize::from(width);
        if self.remaining_bits() < width {
            return Err(BurliError::Format("unexpected end of Brotli input"));
        }

        self.bit_pos += width;
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
        (0..self.remaining_bits()).all(|offset| {
            let absolute = self.bit_pos + offset;
            ((self.input[absolute / 8] >> (absolute % 8)) & 1) == 0
        })
    }
}

#[cfg(feature = "alloc")]
#[derive(Clone, Debug, Default)]
pub struct BitWriter {
    output: Vec<u8>,
    bit_len: usize,
}

#[cfg(feature = "alloc")]
impl BitWriter {
    pub const fn new() -> Self {
        Self {
            output: Vec::new(),
            bit_len: 0,
        }
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            output: Vec::with_capacity(capacity),
            bit_len: 0,
        }
    }

    pub fn written_bits(&self) -> usize {
        self.bit_len
    }

    pub fn write_bits(&mut self, width: u8, value: u64) -> Result<()> {
        if width > MAX_BITS_PER_OP {
            return Err(BurliError::Format("bit write width exceeds 56 bits"));
        }

        let width = usize::from(width);
        let target_bits = self
            .bit_len
            .checked_add(width)
            .ok_or(BurliError::Format("Brotli output bit length overflow"))?;
        let target_len = target_bits.div_ceil(8);
        if self.output.len() < target_len {
            self.output.resize(target_len, 0);
        }

        for offset in 0..width {
            if ((value >> offset) & 1) != 0 {
                let absolute = self.bit_len + offset;
                self.output[absolute / 8] |= 1 << (absolute % 8);
            }
        }

        self.bit_len = target_bits;
        Ok(())
    }

    pub fn align_to_byte(&mut self) -> Result<()> {
        let padding = (8 - (self.bit_len % 8)) % 8;
        self.write_bits(padding as u8, 0)
    }

    pub fn write_aligned_bytes(&mut self, bytes: &[u8]) -> Result<()> {
        self.align_to_byte()?;
        self.output.extend_from_slice(bytes);
        self.bit_len = self
            .bit_len
            .checked_add(bytes.len() * 8)
            .ok_or(BurliError::Format("Brotli output bit length overflow"))?;
        Ok(())
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.output
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
}
