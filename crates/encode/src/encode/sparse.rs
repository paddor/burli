use alloc::{vec, vec::Vec};

use super::{INITIAL_LAST_DISTANCE, MAX_META_BLOCK_SIZE, Token, match_len, read_u64_le, tune};

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
    let store_uncompressed = sample.duplicate_6_count <= tune::LOW_COMPRESS_DUP6_STORE_MAX
        && sample.zero_count * tune::LOW_COMPRESS_ZERO_RATIO_DEN
            <= sample.len * tune::LOW_COMPRESS_ZERO_RATIO_NUM
        && sample.printable_count * tune::LOW_COMPRESS_PRINTABLE_RATIO_DEN
            <= sample.len * tune::LOW_COMPRESS_PRINTABLE_RATIO_NUM;
    Decision {
        sample: Some(sample),
        store_uncompressed,
    }
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

fn literal_only(input_len: usize) -> Vec<Token> {
    if input_len == 0 {
        return Vec::new();
    }
    vec![Token {
        insert_start: 0,
        insert_len: input_len,
        copy_len: 0,
        copy_len_code: 0,
        distance: 0,
        distance_code: None,
        use_last_distance: false,
    }]
}

fn sample(input: &[u8]) -> Sample {
    const TABLE_BITS: usize = 12;
    const TABLE_SIZE: usize = 1 << TABLE_BITS;
    const TABLE_MASK: usize = TABLE_SIZE - 1;
    const EMPTY: u16 = u16::MAX;

    debug_assert!(input.len() >= tune::LOW_COMPRESS_SAMPLE_BYTES);
    let sample_len = input.len().min(tune::LOW_COMPRESS_SAMPLE_BYTES);
    let mut table = [EMPTY; TABLE_SIZE];
    let mut matches = 0_usize;
    let mut zeros = 0_usize;
    let mut printable = 0_usize;
    let mut miss_streak = 0_usize;
    let mut max_miss_streak = 0_usize;
    let mut samples = 0_usize;
    let mut pos = 0_usize;

    while pos + 8 <= sample_len {
        let byte = input[pos];
        let word = read_u64_le(input, pos);
        zeros += usize::from(byte == 0);
        printable +=
            usize::from(byte.is_ascii_graphic() || matches!(byte, b'\t' | b'\n' | b'\r' | b' '));
        samples += 1;
        let key = sample_hash6::<TABLE_BITS>(word) & TABLE_MASK;
        let previous = table[key];
        if previous != EMPTY && is_match6(input, usize::from(previous), pos) {
            matches += 1;
            miss_streak = 0;
        } else {
            miss_streak += 1;
            max_miss_streak = max_miss_streak.max(miss_streak);
        }
        debug_assert!(pos <= u16::MAX as usize);
        table[key] = pos as u16;
        pos += tune::LOW_COMPRESS_SAMPLE_STEP;
    }

    Sample {
        duplicate_6_count: matches,
        zero_count: zeros,
        printable_count: printable,
        max_miss_streak,
        len: samples,
    }
}

#[inline(always)]
fn sample_hash6<const TABLE_BITS: usize>(word: u64) -> usize {
    ((word << 16).wrapping_mul(0x1e35_a7bd) >> (64 - TABLE_BITS)) as usize
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
