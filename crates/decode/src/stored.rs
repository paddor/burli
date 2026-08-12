use alloc::vec::Vec;

use crate::compressed::DistanceRing;
use burli_core::{
    BurliError, DecompressError,
    bits::BitReader,
    format::{MAX_WINDOW_BITS, MIN_WINDOW_BITS},
};

const MAX_META_BLOCK_SIZE: usize = 1 << 24;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MetaBlockHeader {
    LastEmpty,
    Metadata { len: usize, is_last: bool },
    Uncompressed { len: usize },
    Compressed { len: usize, is_last: bool },
}

pub fn decompress_with_limit(
    input: &[u8],
    max_output_size: usize,
) -> Result<Vec<u8>, DecompressError> {
    let mut reader = BitReader::new(input);
    let window_bits = read_window_bits(&mut reader)?;
    let mut output = Vec::new();
    let mut distances = DistanceRing::new();

    loop {
        match read_meta_block_header(&mut reader)? {
            MetaBlockHeader::LastEmpty => {
                finish_stream(&reader)?;
                return Ok(output);
            }
            MetaBlockHeader::Metadata { len, is_last } => {
                reader.read_zero_padding_to_byte()?;
                let _metadata = reader.read_aligned_bytes(len)?;
                if is_last {
                    finish_stream(&reader)?;
                    return Ok(output);
                }
            }
            MetaBlockHeader::Uncompressed { len } => {
                let needed = output.len().saturating_add(len);
                if needed > max_output_size {
                    return Err(BurliError::OutputLimitExceeded {
                        limit: max_output_size,
                        needed,
                    });
                }

                reader.read_zero_padding_to_byte()?;
                let bytes = reader.read_aligned_bytes(len)?;
                output.extend_from_slice(bytes);
            }
            MetaBlockHeader::Compressed { len, is_last } => {
                crate::compressed::decode_meta_block(
                    &mut reader,
                    &mut output,
                    len,
                    max_output_size,
                    window_bits,
                    &mut distances,
                )?;
                if is_last {
                    finish_stream(&reader)?;
                    return Ok(output);
                }
            }
        }
    }
}

fn read_window_bits(reader: &mut BitReader<'_>) -> Result<u8, DecompressError> {
    if !reader.read_bit()? {
        return Ok(16);
    }

    let high = reader.read_bits(3)? as u8;
    if high != 0 {
        return Ok(17 + high);
    }

    let low = reader.read_bits(3)? as u8;
    if low == 1 {
        return Err(BurliError::Format(
            "large-window Brotli streams are not supported",
        ));
    }
    if low != 0 {
        let window_bits = 8 + low;
        if !(MIN_WINDOW_BITS..=MAX_WINDOW_BITS).contains(&window_bits) {
            return Err(BurliError::InvalidWindowBits(window_bits));
        }
        return Ok(window_bits);
    }

    Ok(17)
}

fn read_meta_block_header(reader: &mut BitReader<'_>) -> Result<MetaBlockHeader, DecompressError> {
    let is_last = reader.read_bit()?;
    if is_last && reader.read_bit()? {
        return Ok(MetaBlockHeader::LastEmpty);
    }

    let nibbles_code = reader.read_bits(2)? as u8;
    if nibbles_code == 3 {
        if reader.read_bit()? {
            return Err(BurliError::Format("reserved Brotli metadata bit set"));
        }
        return Ok(MetaBlockHeader::Metadata {
            len: read_metadata_len(reader)?,
            is_last,
        });
    }

    let len = read_meta_block_len(reader, nibbles_code)?;
    if is_last {
        return Ok(MetaBlockHeader::Compressed { len, is_last });
    }

    if reader.read_bit()? {
        Ok(MetaBlockHeader::Uncompressed { len })
    } else {
        Ok(MetaBlockHeader::Compressed { len, is_last })
    }
}

fn read_meta_block_len(
    reader: &mut BitReader<'_>,
    nibbles_code: u8,
) -> Result<usize, DecompressError> {
    let size_nibbles = usize::from(nibbles_code) + 4;
    let mut len_minus_one = 0_usize;

    for index in 0..size_nibbles {
        let nibble = reader.read_bits(4)? as usize;
        if index + 1 == size_nibbles && size_nibbles > 4 && nibble == 0 {
            return Err(BurliError::Format("exuberant Brotli meta-block length"));
        }
        len_minus_one |= nibble << (index * 4);
    }

    let len = len_minus_one + 1;
    if len > MAX_META_BLOCK_SIZE {
        return Err(BurliError::Format("Brotli meta-block exceeds 16 MiB"));
    }

    Ok(len)
}

fn read_metadata_len(reader: &mut BitReader<'_>) -> Result<usize, DecompressError> {
    let size_bytes = reader.read_bits(2)? as usize;
    if size_bytes == 0 {
        return Ok(0);
    }

    let mut len_minus_one = 0_usize;
    for index in 0..size_bytes {
        let byte = reader.read_bits(8)? as usize;
        if index + 1 == size_bytes && size_bytes > 1 && byte == 0 {
            return Err(BurliError::Format("exuberant Brotli metadata block length"));
        }
        len_minus_one |= byte << (index * 8);
    }

    Ok(len_minus_one + 1)
}

fn finish_stream(reader: &BitReader<'_>) -> Result<(), DecompressError> {
    if reader.remaining_bits() >= 8 {
        return Err(BurliError::Format("trailing bytes after Brotli stream"));
    }
    if !reader.remaining_bits_are_zero() {
        return Err(BurliError::Format("non-zero trailing Brotli padding"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_empty_stream() {
        assert_eq!(decompress_with_limit(&[0x06], usize::MAX).unwrap(), b"");
    }

    #[test]
    fn decodes_uncompressed_google_test_stream_shape() {
        let encoded = [
            0x21, 0x03, 0x20, 0x00, 0x08, b'h', b'e', b'l', b'l', b'o', 0x03,
        ];

        assert_eq!(
            decompress_with_limit(&encoded, usize::MAX).unwrap(),
            b"hello"
        );
    }

    #[test]
    fn enforces_output_limit_before_copy() {
        let encoded = [
            0x21, 0x03, 0x20, 0x00, 0x08, b'h', b'e', b'l', b'l', b'o', 0x03,
        ];

        assert_eq!(
            decompress_with_limit(&encoded, 4),
            Err(BurliError::OutputLimitExceeded {
                limit: 4,
                needed: 5
            })
        );
    }

    #[test]
    fn rejects_trailing_bytes() {
        assert!(matches!(
            decompress_with_limit(&[0x06, 0x00], usize::MAX),
            Err(BurliError::Format("trailing bytes after Brotli stream"))
        ));
    }
}
