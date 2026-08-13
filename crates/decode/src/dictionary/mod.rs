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
}
