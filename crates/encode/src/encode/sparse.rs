use alloc::{vec, vec::Vec};

use super::{
    INITIAL_LAST_DISTANCE, MAX_META_BLOCK_SIZE, Token, literal_only, match_len, read_u64_le, tune,
};

#[derive(Clone, Copy, Debug)]
pub(super) struct Decision {
    pub(super) sample: Option<Sample>,
    pub(super) store_uncompressed: bool,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Sample {
    pub(super) duplicate_6_count: usize,
    pub(super) zero_count: usize,
    pub(super) printable_count: usize,
    pub(super) max_miss_streak: usize,
    pub(super) len: usize,
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Q1Skip {
    None,
    Store,
    Moderate,
}

#[cfg(test)]
pub(super) fn should_accelerate(input: &[u8]) -> bool {
    decision(input).store_uncompressed
}

pub(super) fn decision(input: &[u8]) -> Decision {
    if input.len() < tune::LOW_COMPRESS_SAMPLE_MIN_INPUT {
        return Decision {
            sample: None,
            store_uncompressed: false,
        };
    }
    let sample = sample(input);
    Decision {
        sample: Some(sample),
        store_uncompressed: is_low_compressibility(sample),
    }
}

/// Whether matching is unlikely to help `input`. Samples the whole block,
/// so a low-compressibility start does not decide a long block alone.
pub(super) fn is_low_compressibility_block(input: &[u8]) -> bool {
    if input.len() < tune::LOW_COMPRESS_SAMPLE_MIN_INPUT {
        return small_store(input);
    }
    let step = input.len() / tune::LOW_COMPRESS_SAMPLE_COUNT;
    !starts_as_text(input, step) && is_low_compressibility(sample_with::<{ 1 << 12 }>(input, step))
}

/// Whether an input shorter than `LOW_COMPRESS_SAMPLE_MIN_INPUT` looks like
/// low-compressibility binary data, by the same limits as [`decision`].
///
/// Takes about `LOW_COMPRESS_SMALL_SAMPLES` samples, but samples at least
/// every `LOW_COMPRESS_SMALL_MIN_STEP` bytes. More samples did not change
/// which corpus slices get stored.
pub(super) fn small_store(input: &[u8]) -> bool {
    debug_assert!(input.len() < tune::LOW_COMPRESS_SAMPLE_MIN_INPUT);
    let step =
        (input.len() / tune::LOW_COMPRESS_SMALL_SAMPLES).max(tune::LOW_COMPRESS_SMALL_MIN_STEP);
    if starts_as_text(input, step) {
        return false;
    }
    let sample = sample_with::<{ tune::LOW_COMPRESS_SMALL_TABLE_SIZE }>(input, step);
    sample.len != 0 && is_low_compressibility(sample)
}

/// Whether nearly all of the first samples are text bytes. Text is most
/// input, and this rejects it without setting up the hash table. On the
/// benchmark corpus it never rejected an input that the full sample stores.
fn starts_as_text(input: &[u8], step: usize) -> bool {
    let mut text = 0_usize;
    let mut samples = 0_usize;
    let mut pos = 0_usize;
    while samples < tune::LOW_COMPRESS_TEXT_PREFIX_SAMPLES && pos + 8 <= input.len() {
        text += usize::from(BYTE_CLASS[usize::from(input[pos])] >> 1);
        samples += 1;
        pos += step;
    }
    text >= tune::LOW_COMPRESS_TEXT_PREFIX_MIN
}

/// Few repeated 6-byte strings, few zero bytes, and mostly non-text bytes.
/// The repeat limit is `LOW_COMPRESS_DUP6_STORE_MAX` per
/// `LOW_COMPRESS_SAMPLE_COUNT` samples.
fn is_low_compressibility(sample: Sample) -> bool {
    sample.duplicate_6_count * tune::LOW_COMPRESS_SAMPLE_COUNT
        <= tune::LOW_COMPRESS_DUP6_STORE_MAX * sample.len
        && sample.zero_count * tune::LOW_COMPRESS_ZERO_RATIO_DEN
            <= sample.len * tune::LOW_COMPRESS_ZERO_RATIO_NUM
        && sample.printable_count * tune::LOW_COMPRESS_PRINTABLE_RATIO_DEN
            <= sample.len * tune::LOW_COMPRESS_PRINTABLE_RATIO_NUM
}

pub(super) fn q0_store_block(input_base: usize, allow_cross_collector_shortcuts: bool) -> bool {
    let block_in_group = (input_base >> tune::Q0_LOW_COMPRESS_STORE_BLOCK_BITS)
        & tune::Q0_LOW_COMPRESS_STORE_BLOCK_MASK;
    allow_cross_collector_shortcuts
        || (tune::Q0_LOW_COMPRESS_STORE_BLOCKS & (1 << block_in_group)) != 0
}

#[cfg(test)]
pub(super) fn q1_skip(input: &[u8]) -> Q1Skip {
    let decision = decision(input);
    if !decision.store_uncompressed {
        return Q1Skip::None;
    }
    let Some(sample) = decision.sample else {
        return Q1Skip::None;
    };
    if sample.duplicate_6_count > 2 {
        Q1Skip::Moderate
    } else {
        Q1Skip::Store
    }
}

#[cfg(test)]
pub(super) const fn q1_store_block(
    _input_base: usize,
    _allow_cross_collector_shortcuts: bool,
) -> bool {
    false
}

pub(super) fn collect_tokens(
    input: &[u8],
    max_backward_distance: usize,
    stride: usize,
) -> Vec<Token> {
    debug_assert!(stride != 0);
    if input.len() < 8 {
        return literal_only(input.len());
    }

    const TABLE_BITS: usize = 16;
    const TABLE_SIZE: usize = 1 << TABLE_BITS;
    const TABLE_MASK: usize = TABLE_SIZE - 1;
    const EMPTY: u32 = u32::MAX;

    let mut table = vec![EMPTY; TABLE_SIZE];
    let mut tokens = Vec::new();
    let mut pos = 0_usize;
    let mut insert_start = 0_usize;
    let mut last_distance = INITIAL_LAST_DISTANCE;
    let len_limit = input.len().saturating_sub(8);
    let max_backward_distance = max_backward_distance.min((1 << 24) - 16);

    while pos <= len_limit {
        let key = hash6(read_u64_le(input, pos)) & TABLE_MASK;
        let previous = if pos >= last_distance && is_match6(input, pos - last_distance, pos) {
            table[key] = pos as u32;
            Some(pos - last_distance)
        } else {
            let candidate = table[key];
            table[key] = pos as u32;
            if candidate == EMPTY {
                None
            } else {
                let candidate = candidate as usize;
                (candidate < pos
                    && pos - candidate <= max_backward_distance
                    && is_match6(input, candidate, pos))
                .then_some(candidate)
            }
        };

        if let Some(previous) = previous {
            let distance = pos - previous;
            let max_copy_len = (MAX_META_BLOCK_SIZE - (pos - insert_start)).min(input.len() - pos);
            if max_copy_len >= 6 {
                let copy_len = 6 + match_len(input, previous + 6, pos + 6, max_copy_len - 6);
                let token = Token {
                    insert_start,
                    insert_len: pos - insert_start,
                    copy_len,
                    copy_len_code: 0,
                    distance,
                    distance_code: None,
                    use_last_distance: false,
                };
                tokens.push(token);
                pos += copy_len;
                insert_start = pos;
                last_distance = distance;
                pos = pos.saturating_add(stride - 1) / stride * stride;
                continue;
            }
        }

        pos += stride;
    }

    if insert_start < input.len() {
        tokens.push(Token {
            insert_start,
            insert_len: input.len() - insert_start,
            copy_len: 0,
            copy_len_code: 0,
            distance: 0,
            distance_code: None,
            use_last_distance: false,
        });
    }
    tokens
}

fn sample(input: &[u8]) -> Sample {
    debug_assert!(input.len() >= tune::LOW_COMPRESS_SAMPLE_BYTES);
    let sample_len = input.len().min(tune::LOW_COMPRESS_SAMPLE_BYTES);
    sample_with::<{ 1 << 12 }>(&input[..sample_len], tune::LOW_COMPRESS_SAMPLE_STEP)
}

/// Samples every `step` bytes of `input`. Takes fewer than `u16::MAX`
/// samples.
///
/// The loop has no data-dependent branches. Sampled bytes of binary input
/// made the old zero, text, and match branches mispredict about once per
/// sample.
fn sample_with<const TABLE_SIZE: usize>(input: &[u8], step: usize) -> Sample {
    let table_mask = TABLE_SIZE - 1;
    let table_bits = TABLE_SIZE.trailing_zeros() as usize;

    debug_assert!(TABLE_SIZE.is_power_of_two());
    debug_assert!(input.len() / step < usize::from(u16::MAX));
    let sample_len = input.len();
    // Slots hold the sample index plus one. Zero marks an empty slot.
    let mut table = [0_u16; TABLE_SIZE];
    let mut matches = 0_usize;
    let mut zeros = 0_usize;
    let mut printable = 0_usize;
    let mut miss_streak = 0_usize;
    let mut max_miss_streak = 0_usize;
    let mut samples = 0_usize;
    let mut pos = 0_usize;

    while pos + 8 <= sample_len {
        let word = read_u64_le(input, pos);
        let class = BYTE_CLASS[usize::from(word as u8)];
        zeros += usize::from(class & ZERO_BYTE);
        printable += usize::from(class >> 1);
        samples += 1;
        let key = sample_hash6(word, table_bits) & table_mask;
        let slot = usize::from(table[key]);
        let previous = read_u64_le(input, slot.saturating_sub(1) * step);
        let matched = (slot != 0) & ((previous ^ word) << 16 == 0);
        matches += usize::from(matched);
        miss_streak = if matched { 0 } else { miss_streak + 1 };
        max_miss_streak = max_miss_streak.max(miss_streak);
        table[key] = samples as u16;
        pos += step;
    }

    Sample {
        duplicate_6_count: matches,
        zero_count: zeros,
        printable_count: printable,
        max_miss_streak,
        len: samples,
    }
}

const ZERO_BYTE: u8 = 1;
const TEXT_BYTE: u8 = 2;

/// `ZERO_BYTE` or `TEXT_BYTE` for each byte value. Text bytes are ASCII
/// graphic characters, tab, line feed, carriage return, and space.
const BYTE_CLASS: [u8; 256] = byte_classes();

const fn byte_classes() -> [u8; 256] {
    let mut classes = [0_u8; 256];
    classes[0] = ZERO_BYTE;
    let mut byte = 0;
    while byte < 256 {
        let value = byte as u8;
        if value.is_ascii_graphic() || matches!(value, b'\t' | b'\n' | b'\r' | b' ') {
            classes[byte] = TEXT_BYTE;
        }
        byte += 1;
    }
    classes
}

#[inline(always)]
fn sample_hash6(word: u64, table_bits: usize) -> usize {
    ((word << 16).wrapping_mul(0x1e35_a7bd) >> (64 - table_bits)) as usize
}

#[inline(always)]
fn hash6(word: u64) -> usize {
    ((word << 16).wrapping_mul(0x1e35_a7bd) >> 48) as usize
}

#[inline(always)]
fn is_match6(input: &[u8], previous: usize, pos: usize) -> bool {
    let diff = read_u64_le(input, previous) ^ read_u64_le(input, pos);
    diff.trailing_zeros() >= 48
}
