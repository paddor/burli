use alloc::vec::Vec;

use burli_core::dictionary::{
    kBrotliDictionary, kBrotliDictionaryOffsetsByLength, kBrotliDictionarySizeBitsByLength,
    kBrotliMaxDictionaryWordLength, kBrotliMinDictionaryWordLength,
};

use super::{
    INITIAL_LAST_DISTANCE, MIN_MATCH_BYTES, Token, distance_code, match_len, read_u32_le,
    read_u64_le, static_dictionary_hash::kStaticDictionaryHash, token_supports_last_distance,
};

const HASH_MUL: u64 = 0x1e35_a7bd_1e35_a7bd;
const HASH_MUL32: u32 = 0x1e35_a7bd;
const NO_POSITION: u32 = u32::MAX;
const LITERAL_BYTE_SCORE: usize = 135;
const DISTANCE_BIT_PENALTY: usize = 30;
const SCORE_BASE: usize = DISTANCE_BIT_PENALTY * 8 * core::mem::size_of::<usize>();
const MIN_SCORE: usize = SCORE_BASE + 100;
const LAZY_SCORE_DIFF: usize = 175;
const SPARSE_SEARCH_WINDOW: usize = 64;
const LARGE_INPUT_THRESHOLD: usize = 1 << 20;
const CUTOFF_TRANSFORMS_COUNT: usize = 10;
const CUTOFF_TRANSFORMS: u64 = 0x071b_520a_da2d_3200;

#[derive(Clone, Copy, Debug)]
struct Match {
    len: usize,
    len_code: usize,
    distance: usize,
    score: usize,
}

#[derive(Clone, Copy, Debug)]
struct SearchParams {
    max_backward_distance: usize,
    best_len_in: usize,
    min_score: usize,
}

#[derive(Clone, Debug)]
pub(super) struct Workspace {
    dist_cache: [usize; 4],
}

impl Default for Workspace {
    fn default() -> Self {
        Self {
            dist_cache: [INITIAL_LAST_DISTANCE, 11, 15, 16],
        }
    }
}

impl Workspace {
    pub(super) fn reset(&mut self) {
        self.dist_cache = [INITIAL_LAST_DISTANCE, 11, 15, 16];
    }
}

pub(super) fn collect(
    input: &[u8],
    max_backward_distance: usize,
    workspace: &mut Workspace,
) -> Vec<Token> {
    if input.len() <= 1024 {
        collect_with_params::<8, 4, 4, 4>(input, max_backward_distance, workspace)
    } else if input.len() >= LARGE_INPUT_THRESHOLD {
        collect_with_params::<15, 4, 5, 8>(input, max_backward_distance, workspace)
    } else {
        collect_with_params::<14, 4, 4, 4>(input, max_backward_distance, workspace)
    }
}

fn collect_with_params<
    const BUCKET_BITS: usize,
    const BLOCK_BITS: usize,
    const HASH_LEN: usize,
    const HASH_READ_LEN: usize,
>(
    input: &[u8],
    max_backward_distance: usize,
    workspace: &mut Workspace,
) -> Vec<Token> {
    if input.len() < HASH_READ_LEN {
        return literal_only(input.len());
    }

    let bucket_size = 1 << BUCKET_BITS;
    let block_size = 1 << BLOCK_BITS;
    let mut counts = vec![0_u32; bucket_size];
    let mut buckets = vec![NO_POSITION; bucket_size * block_size];
    let mut tokens = Vec::new();
    let mut pos = 0_usize;
    let mut insert_start = 0_usize;
    let mut dist_cache = workspace.dist_cache;
    let pos_end = input.len();
    let store_end = input.len().saturating_sub(HASH_READ_LEN - 1);
    let mut apply_sparse_search = SPARSE_SEARCH_WINDOW;

    while pos + HASH_READ_LEN <= pos_end {
        let max_len = pos_end - pos;
        let Some(mut found) = find_match::<BUCKET_BITS, BLOCK_BITS, HASH_LEN, HASH_READ_LEN>(
            input,
            &mut counts,
            &mut buckets,
            pos,
            max_len,
            dist_cache,
            SearchParams {
                max_backward_distance,
                best_len_in: 0,
                min_score: MIN_SCORE,
            },
        ) else {
            pos += 1;
            if pos > apply_sparse_search {
                pos = skip_sparse::<BUCKET_BITS, BLOCK_BITS, HASH_LEN, HASH_READ_LEN>(
                    input,
                    &mut counts,
                    &mut buckets,
                    pos,
                    pos_end,
                    apply_sparse_search,
                );
            }
            continue;
        };

        let mut delayed = 0;
        while delayed < 4 && pos + 1 + HASH_READ_LEN <= pos_end {
            let lazy_pos = pos + 1;
            let lazy_max_len = pos_end - lazy_pos;
            let best_len_in = found.len.saturating_sub(1).min(lazy_max_len);
            if let Some(next) = find_match::<BUCKET_BITS, BLOCK_BITS, HASH_LEN, HASH_READ_LEN>(
                input,
                &mut counts,
                &mut buckets,
                lazy_pos,
                lazy_max_len,
                dist_cache,
                SearchParams {
                    max_backward_distance,
                    best_len_in,
                    min_score: MIN_SCORE,
                },
            ) && next.score >= found.score + LAZY_SCORE_DIFF
            {
                pos += 1;
                found = next;
                delayed += 1;
                continue;
            }
            break;
        }

        apply_sparse_search = pos + 2 * found.len + SPARSE_SEARCH_WINDOW;
        let max_backward_at_pos = pos.min(max_backward_distance);
        let distance_code = compute_distance_code(found.distance, max_backward_at_pos, dist_cache);
        let mut token = Token {
            insert_start,
            insert_len: pos - insert_start,
            copy_len: found.len,
            copy_len_code: found.len_code,
            distance: found.distance,
            distance_code: (distance_code < 16).then_some(distance_code as u16),
            use_last_distance: false,
        };
        if distance_code == 0 && token_supports_last_distance(token) {
            token.use_last_distance = true;
        }
        tokens.push(token);
        if found.distance <= max_backward_at_pos && distance_code > 0 {
            dist_cache[3] = dist_cache[2];
            dist_cache[2] = dist_cache[1];
            dist_cache[1] = dist_cache[0];
            dist_cache[0] = found.distance;
        }
        store_range::<BUCKET_BITS, BLOCK_BITS, HASH_LEN, HASH_READ_LEN>(
            input,
            &mut counts,
            &mut buckets,
            pos + 2,
            (pos + found.len).min(store_end),
        );
        pos += found.len;
        insert_start = pos;
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

    workspace.dist_cache = dist_cache;
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

fn find_match<
    const BUCKET_BITS: usize,
    const BLOCK_BITS: usize,
    const HASH_LEN: usize,
    const HASH_READ_LEN: usize,
>(
    input: &[u8],
    counts: &mut [u32],
    buckets: &mut [u32],
    pos: usize,
    max_len: usize,
    dist_cache: [usize; 4],
    params: SearchParams,
) -> Option<Match> {
    let key = hash::<BUCKET_BITS, HASH_LEN, HASH_READ_LEN>(input, pos);
    let block_size = 1_usize << BLOCK_BITS;
    let block_mask = block_size - 1;
    let bucket_start = key << BLOCK_BITS;
    let best_check = params.best_len_in.min(max_len.saturating_sub(1));
    let mut best_score = params.min_score;
    let mut best_len = best_check;
    let mut out = None;
    let dictionary_base = pos.min(params.max_backward_distance);

    for (index, &distance) in dist_cache.iter().enumerate() {
        if distance == 0 || distance > params.max_backward_distance || pos < distance {
            continue;
        }
        let previous = pos - distance;
        if input[previous + best_check] != input[pos + best_check] {
            continue;
        }
        let len = match_len(input, previous, pos, max_len);
        if len < 3 && !(len == 2 && index < 2) {
            continue;
        }
        let mut score = score_last_distance(len);
        if index != 0 {
            score = score.saturating_sub(last_distance_penalty(index));
        }
        if score > best_score {
            best_len = len;
            best_score = score;
            out = Some(Match {
                len,
                len_code: len,
                distance,
                score,
            });
        }
    }

    if best_len < 3 {
        best_len = 3;
    }

    if best_len < max_len {
        let first4 = read_u32_le(input, pos);
        let count = counts[key] as usize;
        let down = count.saturating_sub(block_size);
        let mut index = count;
        while index > down {
            index -= 1;
            let previous = buckets[bucket_start + (index & block_mask)] as usize;
            if previous == NO_POSITION as usize || previous >= pos {
                continue;
            }
            let distance = pos - previous;
            if distance > params.max_backward_distance {
                break;
            }
            if previous + best_len >= input.len() || pos + best_len >= input.len() {
                break;
            }
            let compare_pos = pos + best_len - 3;
            let compare_previous = previous + best_len - 3;
            if read_u32_le(input, compare_pos) != read_u32_le(input, compare_previous) {
                continue;
            }
            if read_u32_le(input, previous) != first4 {
                continue;
            }

            let len = if max_len > 4 {
                match_len(input, previous + 4, pos + 4, max_len - 4) + 4
            } else {
                match_len(input, previous, pos, max_len)
            };
            if len < MIN_MATCH_BYTES {
                continue;
            }
            let score = score_distance(len, distance);
            if score > best_score {
                best_len = len;
                best_score = score;
                out = Some(Match {
                    len,
                    len_code: len,
                    distance,
                    score,
                });
            }
        }
    }

    store::<BUCKET_BITS, BLOCK_BITS, HASH_LEN, HASH_READ_LEN>(input, counts, buckets, pos);
    find_static_dictionary_identity(input, pos, max_len, dictionary_base, best_score).or(out)
}

fn find_static_dictionary_identity(
    input: &[u8],
    pos: usize,
    max_len: usize,
    dictionary_base: usize,
    min_score: usize,
) -> Option<Match> {
    let key = (hash14(input, pos) << 1) as usize;
    let mut best_score = min_score;
    let mut best = None;

    for item in kStaticDictionaryHash[key..key + 2]
        .iter()
        .map(|&item| usize::from(item))
    {
        if item == 0 {
            continue;
        }

        let len = item & 0x1f;
        let min_len = usize::from(kBrotliMinDictionaryWordLength);
        let max_dict_len = usize::from(kBrotliMaxDictionaryWordLength);
        if len < min_len || len > max_dict_len || len > max_len {
            continue;
        }

        let dist = item >> 5;
        let Some(offset) = usize::try_from(kBrotliDictionaryOffsetsByLength[len])
            .ok()
            .and_then(|offset| offset.checked_add(len.checked_mul(dist)?))
        else {
            continue;
        };
        let Some(word) = kBrotliDictionary.get(offset..offset.saturating_add(len)) else {
            continue;
        };
        let match_len = dictionary_match_len(input, pos, word, len);
        if match_len == 0 || match_len + CUTOFF_TRANSFORMS_COUNT <= len {
            continue;
        }

        let size_bits = usize::from(kBrotliDictionarySizeBitsByLength[len]);
        if dist >= (1_usize << size_bits) {
            continue;
        }
        let cut = len - match_len;
        let transform_id = (cut << 2) + ((CUTOFF_TRANSFORMS >> (cut * 6)) & 0x3f) as usize;
        let Some(distance) = dictionary_base
            .checked_add(dist)
            .and_then(|distance| distance.checked_add(transform_id.checked_shl(size_bits as u32)?))
            .and_then(|distance| distance.checked_add(1))
        else {
            continue;
        };
        if distance_code(distance).is_err() {
            continue;
        }
        let score = score_distance(match_len, distance);
        if score >= best_score {
            best_score = score;
            best = Some(Match {
                len: match_len,
                len_code: len,
                distance,
                score,
            });
        }
    }

    best
}

fn dictionary_match_len(input: &[u8], pos: usize, word: &[u8], max_len: usize) -> usize {
    let Some(candidate) = input.get(pos..pos.saturating_add(max_len)) else {
        return 0;
    };
    candidate
        .iter()
        .zip(word)
        .take_while(|(left, right)| left == right)
        .count()
}

fn skip_sparse<
    const BUCKET_BITS: usize,
    const BLOCK_BITS: usize,
    const HASH_LEN: usize,
    const HASH_READ_LEN: usize,
>(
    input: &[u8],
    counts: &mut [u32],
    buckets: &mut [u32],
    mut pos: usize,
    pos_end: usize,
    start: usize,
) -> usize {
    let margin = HASH_READ_LEN - 1;
    if pos > start + 4 * SPARSE_SEARCH_WINDOW {
        let pos_jump = (pos + 16).min(pos_end.saturating_sub(margin));
        while pos < pos_jump {
            store::<BUCKET_BITS, BLOCK_BITS, HASH_LEN, HASH_READ_LEN>(input, counts, buckets, pos);
            pos += 4;
        }
    } else {
        let pos_jump = (pos + 8).min(pos_end.saturating_sub(margin));
        while pos < pos_jump {
            store::<BUCKET_BITS, BLOCK_BITS, HASH_LEN, HASH_READ_LEN>(input, counts, buckets, pos);
            pos += 2;
        }
    }
    pos
}

fn store_range<
    const BUCKET_BITS: usize,
    const BLOCK_BITS: usize,
    const HASH_LEN: usize,
    const HASH_READ_LEN: usize,
>(
    input: &[u8],
    counts: &mut [u32],
    buckets: &mut [u32],
    start: usize,
    end: usize,
) {
    for pos in start..end {
        store::<BUCKET_BITS, BLOCK_BITS, HASH_LEN, HASH_READ_LEN>(input, counts, buckets, pos);
    }
}

fn store<
    const BUCKET_BITS: usize,
    const BLOCK_BITS: usize,
    const HASH_LEN: usize,
    const HASH_READ_LEN: usize,
>(
    input: &[u8],
    counts: &mut [u32],
    buckets: &mut [u32],
    pos: usize,
) {
    let key = hash::<BUCKET_BITS, HASH_LEN, HASH_READ_LEN>(input, pos);
    let block_size = 1_usize << BLOCK_BITS;
    let block_mask = block_size - 1;
    let index = counts[key] as usize & block_mask;
    buckets[(key << BLOCK_BITS) + index] = pos as u32;
    counts[key] = counts[key].wrapping_add(1);
}

#[inline(always)]
fn hash<const BUCKET_BITS: usize, const HASH_LEN: usize, const HASH_READ_LEN: usize>(
    input: &[u8],
    pos: usize,
) -> usize {
    if HASH_READ_LEN == 4 {
        return (read_u32_le(input, pos).wrapping_mul(HASH_MUL32) >> (32 - BUCKET_BITS)) as usize;
    }
    let word = read_u64_le(input, pos) << (64 - 8 * HASH_LEN);
    (word.wrapping_mul(HASH_MUL) >> (64 - BUCKET_BITS)) as usize
}

fn hash14(input: &[u8], pos: usize) -> u32 {
    read_u32_le(input, pos).wrapping_mul(HASH_MUL32) >> (32 - 14)
}

fn score_distance(len: usize, distance: usize) -> usize {
    SCORE_BASE + LITERAL_BYTE_SCORE * len
        - DISTANCE_BIT_PENALTY * (usize::BITS as usize - 1 - distance.leading_zeros() as usize)
}

fn score_last_distance(len: usize) -> usize {
    SCORE_BASE + LITERAL_BYTE_SCORE * len + 15
}

fn last_distance_penalty(index: usize) -> usize {
    39 + ((0x1ca10_usize >> (index & 0x0e)) & 0x0e)
}

fn compute_distance_code(
    distance: usize,
    max_backward_distance: usize,
    dist_cache: [usize; 4],
) -> usize {
    if distance <= max_backward_distance {
        let distance_plus_3 = distance + 3;
        let offset0 = distance_plus_3.wrapping_sub(dist_cache[0]);
        let offset1 = distance_plus_3.wrapping_sub(dist_cache[1]);
        if distance == dist_cache[0] {
            return 0;
        }
        if distance == dist_cache[1] {
            return 1;
        }
        if offset0 < 7 {
            return (0x0975_0468_usize >> (4 * offset0)) & 0x0f;
        }
        if offset1 < 7 {
            return (0x0fdb_1ace_usize >> (4 * offset1)) & 0x0f;
        }
        if distance == dist_cache[2] {
            return 2;
        }
        if distance == dist_cache[3] {
            return 3;
        }
    }
    distance + 15
}
