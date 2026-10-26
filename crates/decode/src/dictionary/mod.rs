#[cfg(test)]
use alloc::vec::Vec;

use burli_core::{
    BurliError, DecompressError,
    dictionary::{
        kBrotliDictionary, kBrotliDictionaryOffsetsByLength, kBrotliDictionarySizeBitsByLength,
        kBrotliMaxDictionaryWordLength, kBrotliMinDictionaryWordLength,
    },
};

mod transform;
use transform::TransformedWord;

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

/// Copies `copy_len` bytes for a `distance` that reaches into the raw
/// dictionary to `output[pos..]`, and returns the new end. Bytes past the
/// dictionary end continue at the window start. The caller makes sure
/// `output` holds `needed` bytes.
pub(crate) fn copy_raw_lz77(
    output: &mut [u8],
    pos: usize,
    dictionary: RawDictionary<'_>,
    distance: usize,
    max_allowed_distance: usize,
    copy_len: usize,
    needed: usize,
) -> Result<usize, DecompressError> {
    let end = pos
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
    let mut dst = pos + copied_from_dictionary;
    output[pos..dst].copy_from_slice(&dictionary_suffix[..copied_from_dictionary]);

    let mut remaining = copy_len - copied_from_dictionary;
    if remaining == 0 {
        return Ok(end);
    }

    let mut source = pos
        .checked_sub(max_allowed_distance)
        .ok_or(BurliError::Format(
            "Brotli raw dictionary continuation underflow",
        ))?;
    while remaining != 0 {
        let available = dst - source;
        if available == 0 {
            return Err(BurliError::Format(
                "invalid Brotli raw dictionary continuation",
            ));
        }
        let chunk = available.min(remaining);
        output.copy_within(source..source + chunk, dst);
        source += chunk;
        dst += chunk;
        remaining -= chunk;
    }

    debug_assert_eq!(dst, end);
    Ok(end)
}

/// Writes the static dictionary word for `distance` to `output[pos..]`, and
/// returns the new end. The caller makes sure `output` holds `needed` bytes.
///
/// It inlines into the caller's cold path, so a word costs one call.
#[inline(always)]
pub(crate) fn copy_lookup(
    output: &mut [u8],
    pos: usize,
    distance: usize,
    max_allowed_distance: usize,
    copy_len: usize,
    needed: usize,
) -> Result<usize, DecompressError> {
    let (tail, transform_index) = lookup_word(distance, max_allowed_distance, copy_len)?;
    let word = TransformedWord::new(tail, copy_len, transform_index)
        .ok_or(BurliError::Format("invalid Brotli dictionary transform"))?;
    let end = pos
        .checked_add(word.len())
        .ok_or(BurliError::Format("Brotli dictionary copy length overflow"))?;
    if end > needed {
        return Err(BurliError::Format(
            "Brotli dictionary copy exceeds meta-block size",
        ));
    }
    word.write(output, pos)
        .ok_or(BurliError::Format("invalid Brotli dictionary transform"))?;
    Ok(end)
}

#[cfg(test)]
pub(crate) fn lookup(
    distance: usize,
    max_allowed_distance: usize,
    copy_len: usize,
) -> Result<Vec<u8>, DecompressError> {
    let (tail, transform_index) = lookup_word(distance, max_allowed_distance, copy_len)?;
    let word = TransformedWord::new(tail, copy_len, transform_index)
        .ok_or(BurliError::Format("invalid Brotli dictionary transform"))?;
    let mut output = alloc::vec![0; word.len()];

    word.write(&mut output, 0)
        .ok_or(BurliError::Format("invalid Brotli dictionary transform"))?;
    Ok(output)
}

/// Returns the dictionary from the `copy_len`-byte word of `distance` on,
/// and the word's transform.
#[inline(always)]
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
    // At most 2^11 words of at most 24 bytes, so the offset cannot overflow.
    let offset = kBrotliDictionaryOffsetsByLength[copy_len] as usize + word_index * copy_len;
    if offset + copy_len > kBrotliDictionary.len() {
        return Err(BurliError::Format("Brotli dictionary word out of bounds"));
    }
    Ok((&kBrotliDictionary[offset..], transform_index))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_transform_resolves_first_len4_word() {
        assert_eq!(lookup(1, 0, 4).unwrap(), b"time");
    }

    /// `copy_lookup` with room writes fixed-size windows; `lookup` writes
    /// into an exact buffer. Both must agree for every transform, including
    /// the last words of the dictionary.
    #[test]
    fn windowed_words_match_exact_words() {
        let mut output = [0xaa; 64];
        for (copy_len, &size_bits) in kBrotliDictionarySizeBitsByLength.iter().enumerate().skip(4) {
            let words = 1_usize << size_bits;
            for transform in 0..=transform::kNumTransforms as usize {
                for word_index in [0, 1, words / 2, words - 1] {
                    let distance = 1 + ((transform << size_bits) | word_index);
                    let exact = lookup(distance, 0, copy_len);
                    let windowed = copy_lookup(&mut output, 0, distance, 0, copy_len, 48)
                        .map(|end| output[..end].to_vec());
                    assert_eq!(windowed, exact, "len {copy_len} transform {transform}");
                }
            }
        }
    }

    #[test]
    fn rejects_invalid_lengths() {
        assert!(matches!(
            lookup(1, 0, 3),
            Err(BurliError::Format("invalid Brotli dictionary word length"))
        ));
    }

    /// Runs [`copy_raw_lz77`] on `history` followed by zeroed room for
    /// `needed`, and returns the output up to the copy end.
    fn raw_copy(
        history: &[u8],
        dictionary: &[u8],
        distance: usize,
        max_allowed_distance: usize,
        copy_len: usize,
        needed: usize,
    ) -> Vec<u8> {
        let mut output = history.to_vec();
        output.resize(needed, 0);
        let end = copy_raw_lz77(
            &mut output,
            history.len(),
            RawDictionary::new(dictionary),
            distance,
            max_allowed_distance,
            copy_len,
            needed,
        )
        .unwrap();
        output.truncate(end);
        output
    }

    #[test]
    fn raw_lz77_copy_reads_dictionary_suffix() {
        assert_eq!(raw_copy(b"abc", b"012345", 5, 3, 2, 5), b"abc45");
    }

    #[test]
    fn raw_lz77_copy_continues_into_window_beginning() {
        assert_eq!(raw_copy(b"abcdef", b"XYZ", 9, 6, 7, 13), b"abcdefXYZabcd");
    }

    #[test]
    fn raw_lz77_copy_can_overlap_after_dictionary() {
        assert_eq!(raw_copy(b"", b"ab", 2, 0, 6, 6), b"ababab");
    }
}

#[cfg(kani)]
mod verification {
    use super::*;

    #[kani::proof]
    fn rejects_lengths_below_dictionary_minimum() {
        let len = usize::from(kBrotliMinDictionaryWordLength) - 1;

        assert!(matches!(
            lookup_word(1, 0, len),
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
