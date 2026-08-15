use alloc::vec::Vec;

use burli_core::{
    BurliError, DecompressError,
    dictionary::{
        kBrotliDictionary, kBrotliDictionaryOffsetsByLength, kBrotliDictionarySizeBitsByLength,
        kBrotliMaxDictionaryWordLength, kBrotliMinDictionaryWordLength,
    },
};

mod transform;
use transform::{transform_dictionary_word, transformed_dictionary_word_len};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct RawDictionary<'a> {
    bytes: &'a [u8],
}

impl<'a> RawDictionary<'a> {
    pub(crate) const fn empty() -> Self {
        Self { bytes: &[] }
    }

    pub(crate) const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }

    pub(crate) const fn len(self) -> usize {
        self.bytes.len()
    }

    pub(crate) const fn is_empty(self) -> bool {
        self.bytes.is_empty()
    }
}

pub(crate) fn append_raw_lz77_copy(
    output: &mut Vec<u8>,
    dictionary: RawDictionary<'_>,
    distance: usize,
    max_allowed_distance: usize,
    copy_len: usize,
    needed: usize,
) -> Result<(), DecompressError> {
    let end = output
        .len()
        .checked_add(copy_len)
        .ok_or(BurliError::Format("Brotli raw dictionary copy overflow"))?;
    if end > needed {
        return Err(BurliError::Format(
            "Brotli raw dictionary copy exceeds meta-block size",
        ));
    }

    let dictionary_address = dictionary
        .len()
        .checked_add(max_allowed_distance)
        .and_then(|base| base.checked_sub(distance))
        .ok_or(BurliError::Format("invalid Brotli raw dictionary distance"))?;
    let dictionary_suffix = dictionary
        .bytes
        .get(dictionary_address..)
        .ok_or(BurliError::Format("invalid Brotli raw dictionary distance"))?;
    let copied_from_dictionary = copy_len.min(dictionary_suffix.len());
    output.extend_from_slice(&dictionary_suffix[..copied_from_dictionary]);

    let mut remaining = copy_len - copied_from_dictionary;
    if remaining == 0 {
        debug_assert_eq!(output.len(), end);
        return Ok(());
    }

    let mut source = output
        .len()
        .checked_sub(copied_from_dictionary)
        .and_then(|old_len| old_len.checked_sub(max_allowed_distance))
        .ok_or(BurliError::Format(
            "Brotli raw dictionary continuation underflow",
        ))?;
    while remaining != 0 {
        let available = output.len().checked_sub(source).ok_or(BurliError::Format(
            "Brotli raw dictionary continuation underflow",
        ))?;
        if available == 0 {
            return Err(BurliError::Format(
                "invalid Brotli raw dictionary continuation",
            ));
        }
        let chunk = available.min(remaining);
        output.extend_from_within(source..source + chunk);
        source += chunk;
        remaining -= chunk;
    }

    debug_assert_eq!(output.len(), end);
    Ok(())
}

pub(crate) fn append_lookup(
    output: &mut Vec<u8>,
    distance: usize,
    max_allowed_distance: usize,
    copy_len: usize,
    needed: usize,
) -> Result<(), DecompressError> {
    let (word, transform_index) = lookup_word(distance, max_allowed_distance, copy_len)?;
    let transformed_len = transformed_dictionary_word_len(word.len(), transform_index)
        .ok_or(BurliError::Format("invalid Brotli dictionary transform"))?;
    let end = output
        .len()
        .checked_add(transformed_len)
        .ok_or(BurliError::Format("Brotli dictionary copy length overflow"))?;
    if end > needed {
        return Err(BurliError::Format(
            "Brotli dictionary copy exceeds meta-block size",
        ));
    }
    transform_dictionary_word(output, word, transform_index)
        .ok_or(BurliError::Format("invalid Brotli dictionary transform"))?;
    debug_assert_eq!(output.len(), end);
    Ok(())
}

#[cfg(any(test, kani))]
pub(crate) fn lookup(
    distance: usize,
    max_allowed_distance: usize,
    copy_len: usize,
) -> Result<Vec<u8>, DecompressError> {
    let (word, transform_index) = lookup_word(distance, max_allowed_distance, copy_len)?;
    let transformed_len = transformed_dictionary_word_len(word.len(), transform_index)
        .ok_or(BurliError::Format("invalid Brotli dictionary transform"))?;
    let mut output = Vec::with_capacity(transformed_len);

    transform_dictionary_word(&mut output, word, transform_index)
        .ok_or(BurliError::Format("invalid Brotli dictionary transform"))?;
    Ok(output)
}

fn lookup_word(
    distance: usize,
    max_allowed_distance: usize,
    copy_len: usize,
) -> Result<(&'static [u8], usize), DecompressError> {
    let min_len = usize::from(kBrotliMinDictionaryWordLength);
    let max_len = usize::from(kBrotliMaxDictionaryWordLength);
    if copy_len < min_len || copy_len > max_len {
        return Err(BurliError::Format("invalid Brotli dictionary word length"));
    }
    if distance <= max_allowed_distance {
        return Err(BurliError::Format("invalid Brotli dictionary distance"));
    }

    let word_id = distance - max_allowed_distance - 1;
    let size_bits = usize::from(kBrotliDictionarySizeBitsByLength[copy_len]);
    let word_count = 1_usize << size_bits;
    let word_index = word_id & (word_count - 1);
    let transform_index = word_id >> size_bits;
    let base_offset = kBrotliDictionaryOffsetsByLength[copy_len] as usize;
    let offset = base_offset
        .checked_add(word_index.saturating_mul(copy_len))
        .ok_or(BurliError::Format("Brotli dictionary offset overflow"))?;
    let end = offset
        .checked_add(copy_len)
        .ok_or(BurliError::Format("Brotli dictionary word overflow"))?;
    let word = kBrotliDictionary
        .get(offset..end)
        .ok_or(BurliError::Format("Brotli dictionary word out of bounds"))?;
    Ok((word, transform_index))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_transform_resolves_first_len4_word() {
        assert_eq!(lookup(1, 0, 4).unwrap(), b"time");
    }

    #[test]
    fn rejects_invalid_lengths() {
        assert!(matches!(
            lookup(1, 0, 3),
            Err(BurliError::Format("invalid Brotli dictionary word length"))
        ));
    }

    #[test]
    fn raw_lz77_copy_reads_dictionary_suffix() {
        let mut output = b"abc".to_vec();
        let dictionary = RawDictionary::new(b"012345");

        append_raw_lz77_copy(&mut output, dictionary, 5, 3, 2, 5).unwrap();

        assert_eq!(output, b"abc45");
    }

    #[test]
    fn raw_lz77_copy_continues_into_window_beginning() {
        let mut output = b"abcdef".to_vec();
        let dictionary = RawDictionary::new(b"XYZ");

        append_raw_lz77_copy(&mut output, dictionary, 9, 6, 7, 13).unwrap();

        assert_eq!(output, b"abcdefXYZabcd");
    }

    #[test]
    fn raw_lz77_copy_can_overlap_after_dictionary() {
        let mut output = Vec::new();
        let dictionary = RawDictionary::new(b"ab");

        append_raw_lz77_copy(&mut output, dictionary, 2, 0, 6, 6).unwrap();

        assert_eq!(output, b"ababab");
    }
}

#[cfg(kani)]
mod verification {
    use super::*;

    #[kani::proof]
    fn rejects_lengths_below_dictionary_minimum() {
        let len = usize::from(kBrotliMinDictionaryWordLength) - 1;

        assert!(matches!(
            lookup(1, 0, len),
            Err(BurliError::Format("invalid Brotli dictionary word length"))
        ));
    }

    #[kani::proof]
    fn raw_lz77_distance_range_maps_inside_dictionary() {
        let dictionary_len = usize::from(kani::any::<u8>());
        let max_allowed_distance = usize::from(kani::any::<u8>());
        let delta = usize::from(kani::any::<u8>());

        kani::assume(dictionary_len != 0);
        kani::assume((1..=dictionary_len).contains(&delta));

        let distance = max_allowed_distance + delta;
        let dictionary_address = dictionary_len + max_allowed_distance - distance;

        assert!(dictionary_address < dictionary_len);
    }
}
