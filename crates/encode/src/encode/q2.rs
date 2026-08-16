use alloc::{vec, vec::Vec};

use burli_core::dictionary::{
    kBrotliDictionary, kBrotliDictionaryOffsetsByLength, kBrotliDictionarySizeBitsByLength,
    kBrotliMaxDictionaryWordLength, kBrotliMinDictionaryWordLength,
};

use super::{
    INITIAL_LAST_DISTANCE, MIN_MATCH_BYTES, Token, compute_distance_code, distance_code,
    literal_only, match_len, read_u32_le, read_u64_le, score_distance, score_last_distance,
    static_dictionary_hash::kStaticDictionaryHash, token_supports_last_distance, tune,
};

const TABLE_BITS: usize = 16;
const TABLE_SIZE: usize = 1 << TABLE_BITS;
const HASH_LEN: usize = 5;
const HASH_TYPE_LEN: usize = 8;
const STORE_LOOKAHEAD: usize = 8;
const HASH_MUL: u64 = 0x1e35_a7bd_1e35_a7bd;
const NO_POSITION: u32 = u32::MAX;
use super::MIN_SCORE;

const LAZY_SCORE_DIFF: usize = 175;
const SPARSE_SEARCH_WINDOW: usize = 64;
const LONG_MATCH_STORE_THRESHOLD: usize = 64;
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
    input_base: usize,
    max_backward_distance: usize,
    last_distance: usize,
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
    input_base: usize,
    max_backward_distance: usize,
    workspace: &mut Workspace,
) -> Vec<Token> {
    if input.len() > tune::Q2_USE_DICTIONARY_MAX_INPUT {
        collect_with_dictionary::<false>(input, input_base, max_backward_distance, workspace)
    } else {
        collect_with_dictionary::<true>(input, input_base, max_backward_distance, workspace)
    }
}

pub(super) fn collect_without_dictionary_no_lazy(
    input: &[u8],
    max_backward_distance: usize,
    workspace: &mut Workspace,
) -> Vec<Token> {
    collect_with_dictionary_lazy::<false, 0, true, true, 4>(
        input,
        0,
        max_backward_distance,
        workspace,
    )
}

pub(super) fn collect_without_dictionary_no_lazy_sparse_tail(
    input: &[u8],
    max_backward_distance: usize,
    workspace: &mut Workspace,
) -> Vec<Token> {
    collect_with_dictionary_lazy::<false, 0, true, true, 8>(
        input,
        0,
        max_backward_distance,
        workspace,
    )
}

pub(super) fn collect_without_dictionary_no_lazy_sparse_tail_no_last_distance_probe(
    input: &[u8],
    max_backward_distance: usize,
    workspace: &mut Workspace,
) -> Vec<Token> {
    collect_with_dictionary_lazy::<false, 0, false, false, 8>(
        input,
        0,
        max_backward_distance,
        workspace,
    )
}

pub(super) fn collect_without_dictionary_one_lazy(
    input: &[u8],
    max_backward_distance: usize,
    workspace: &mut Workspace,
) -> Vec<Token> {
    collect_with_dictionary_lazy::<false, 1, false, false, 8>(
        input,
        0,
        max_backward_distance,
        workspace,
    )
}

pub(super) fn collect_without_dictionary(
    input: &[u8],
    max_backward_distance: usize,
    workspace: &mut Workspace,
) -> Vec<Token> {
    collect_with_dictionary::<false>(input, 0, max_backward_distance, workspace)
}

fn collect_with_dictionary<const USE_DICTIONARY: bool>(
    input: &[u8],
    input_base: usize,
    max_backward_distance: usize,
    workspace: &mut Workspace,
) -> Vec<Token> {
    collect_with_dictionary_lazy::<USE_DICTIONARY, 4, true, true, 4>(
        input,
        input_base,
        max_backward_distance,
        workspace,
    )
}

fn collect_with_dictionary_lazy<
    const USE_DICTIONARY: bool,
    const MAX_LAZY_MATCHES: usize,
    const PROBE_LAST_DISTANCE: bool,
    const PROBE_LAZY_LAST_DISTANCE: bool,
    const LONG_MATCH_STORE_STEP: usize,
>(
    input: &[u8],
    input_base: usize,
    max_backward_distance: usize,
    workspace: &mut Workspace,
) -> Vec<Token> {
    match table_bits_for_input(input.len()) {
        10 => {
            return collect_with_stack_table_10::<
                USE_DICTIONARY,
                MAX_LAZY_MATCHES,
                PROBE_LAST_DISTANCE,
                PROBE_LAZY_LAST_DISTANCE,
                LONG_MATCH_STORE_STEP,
            >(input, input_base, max_backward_distance, workspace);
        }
        12 => {
            return collect_with_stack_table_12::<
                USE_DICTIONARY,
                MAX_LAZY_MATCHES,
                PROBE_LAST_DISTANCE,
                PROBE_LAZY_LAST_DISTANCE,
                LONG_MATCH_STORE_STEP,
            >(input, input_base, max_backward_distance, workspace);
        }
        16 if !USE_DICTIONARY => {
            return collect_with_stack_table_16::<
                USE_DICTIONARY,
                MAX_LAZY_MATCHES,
                PROBE_LAST_DISTANCE,
                PROBE_LAZY_LAST_DISTANCE,
                LONG_MATCH_STORE_STEP,
            >(input, input_base, max_backward_distance, workspace);
        }
        _ => {}
    }

    let mut table = vec![NO_POSITION; TABLE_SIZE];
    collect_with_table::<
        USE_DICTIONARY,
        TABLE_BITS,
        MAX_LAZY_MATCHES,
        PROBE_LAST_DISTANCE,
        PROBE_LAZY_LAST_DISTANCE,
        LONG_MATCH_STORE_STEP,
    >(
        input,
        input_base,
        max_backward_distance,
        workspace,
        &mut table,
    )
}

#[allow(clippy::large_stack_arrays)]
fn collect_with_stack_table_16<
    const USE_DICTIONARY: bool,
    const MAX_LAZY_MATCHES: usize,
    const PROBE_LAST_DISTANCE: bool,
    const PROBE_LAZY_LAST_DISTANCE: bool,
    const LONG_MATCH_STORE_STEP: usize,
>(
    input: &[u8],
    input_base: usize,
    max_backward_distance: usize,
    workspace: &mut Workspace,
) -> Vec<Token> {
    let mut table = [NO_POSITION; TABLE_SIZE];
    collect_with_table::<
        USE_DICTIONARY,
        16,
        MAX_LAZY_MATCHES,
        PROBE_LAST_DISTANCE,
        PROBE_LAZY_LAST_DISTANCE,
        LONG_MATCH_STORE_STEP,
    >(
        input,
        input_base,
        max_backward_distance,
        workspace,
        &mut table,
    )
}

fn collect_with_stack_table_10<
    const USE_DICTIONARY: bool,
    const MAX_LAZY_MATCHES: usize,
    const PROBE_LAST_DISTANCE: bool,
    const PROBE_LAZY_LAST_DISTANCE: bool,
    const LONG_MATCH_STORE_STEP: usize,
>(
    input: &[u8],
    input_base: usize,
    max_backward_distance: usize,
    workspace: &mut Workspace,
) -> Vec<Token> {
    let mut table = [NO_POSITION; 1 << 10];
    collect_with_table::<
        USE_DICTIONARY,
        10,
        MAX_LAZY_MATCHES,
        PROBE_LAST_DISTANCE,
        PROBE_LAZY_LAST_DISTANCE,
        LONG_MATCH_STORE_STEP,
    >(
        input,
        input_base,
        max_backward_distance,
        workspace,
        &mut table,
    )
}

#[allow(clippy::large_stack_arrays)]
fn collect_with_stack_table_12<
    const USE_DICTIONARY: bool,
    const MAX_LAZY_MATCHES: usize,
    const PROBE_LAST_DISTANCE: bool,
    const PROBE_LAZY_LAST_DISTANCE: bool,
    const LONG_MATCH_STORE_STEP: usize,
>(
    input: &[u8],
    input_base: usize,
    max_backward_distance: usize,
    workspace: &mut Workspace,
) -> Vec<Token> {
    let mut table = [NO_POSITION; 1 << 12];
    collect_with_table::<
        USE_DICTIONARY,
        12,
        MAX_LAZY_MATCHES,
        PROBE_LAST_DISTANCE,
        PROBE_LAZY_LAST_DISTANCE,
        LONG_MATCH_STORE_STEP,
    >(
        input,
        input_base,
        max_backward_distance,
        workspace,
        &mut table,
    )
}

fn collect_with_table<
    const USE_DICTIONARY: bool,
    const TABLE_BITS_FOR_INPUT: usize,
    const MAX_LAZY_MATCHES: usize,
    const PROBE_LAST_DISTANCE: bool,
    const PROBE_LAZY_LAST_DISTANCE: bool,
    const LONG_MATCH_STORE_STEP: usize,
>(
    input: &[u8],
    input_base: usize,
    max_backward_distance: usize,
    workspace: &mut Workspace,
    table: &mut [u32],
) -> Vec<Token> {
    debug_assert_eq!(table.len(), 1_usize << TABLE_BITS_FOR_INPUT);
    if input.len() < HASH_TYPE_LEN {
        return literal_only(input.len());
    }

    let mut tokens = Vec::new();
    let mut pos = 0_usize;
    let mut insert_start = 0_usize;
    let mut dist_cache = workspace.dist_cache;
    let pos_end = input.len();
    let store_end = input.len().saturating_sub(STORE_LOOKAHEAD - 1);
    let mut apply_sparse_search = SPARSE_SEARCH_WINDOW;

    while pos + HASH_TYPE_LEN < pos_end {
        let max_len = pos_end - pos;
        let Some(mut found) = find_match::<USE_DICTIONARY, TABLE_BITS_FOR_INPUT, PROBE_LAST_DISTANCE>(
            input,
            table,
            pos,
            max_len,
            SearchParams {
                input_base,
                max_backward_distance,
                last_distance: dist_cache[0],
                best_len_in: 0,
                min_score: MIN_SCORE,
            },
        ) else {
            pos += 1;
            if pos > apply_sparse_search {
                pos = skip_sparse::<TABLE_BITS_FOR_INPUT>(
                    input,
                    table,
                    pos,
                    pos_end,
                    apply_sparse_search,
                );
            }
            continue;
        };

        let mut delayed = 0;
        while delayed < MAX_LAZY_MATCHES && pos + 1 + HASH_TYPE_LEN < pos_end {
            let lazy_pos = pos + 1;
            let lazy_max_len = pos_end - lazy_pos;
            let best_len_in = found.len.saturating_sub(1).min(lazy_max_len);
            if let Some(next) =
                find_match::<USE_DICTIONARY, TABLE_BITS_FOR_INPUT, PROBE_LAZY_LAST_DISTANCE>(
                    input,
                    table,
                    lazy_pos,
                    lazy_max_len,
                    SearchParams {
                        input_base,
                        max_backward_distance,
                        last_distance: dist_cache[0],
                        best_len_in,
                        min_score: MIN_SCORE,
                    },
                )
                && next.score >= found.score + LAZY_SCORE_DIFF
            {
                pos += 1;
                found = next;
                delayed += 1;
                continue;
            }
            break;
        }

        apply_sparse_search = pos + 2 * found.len + SPARSE_SEARCH_WINDOW;
        let max_backward_at_pos = input_base.saturating_add(pos).min(max_backward_distance);
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
        store_range::<TABLE_BITS_FOR_INPUT, LONG_MATCH_STORE_STEP>(
            input,
            table,
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

#[inline(always)]
fn find_match<
    const USE_DICTIONARY: bool,
    const TABLE_BITS_FOR_INPUT: usize,
    const PROBE_LAST_DISTANCE: bool,
>(
    input: &[u8],
    table: &mut [u32],
    pos: usize,
    max_len: usize,
    params: SearchParams,
) -> Option<Match> {
    let key = table_key::<TABLE_BITS_FOR_INPUT>(input, pos);
    let best_check = params.best_len_in.min(max_len.saturating_sub(1));
    let compare_char = input[pos + best_check];
    let best_score = params.min_score;

    if PROBE_LAST_DISTANCE && pos >= params.last_distance {
        let previous = pos - params.last_distance;
        if input[previous + best_check] == compare_char {
            let len = match_len(input, previous, pos, max_len);
            if len >= MIN_MATCH_BYTES {
                let score = score_last_distance(len);
                if score > best_score {
                    table[key] = pos as u32;
                    return Some(Match {
                        len,
                        len_code: len,
                        distance: params.last_distance,
                        score,
                    });
                }
            }
        }
    }

    let previous = table[key] as usize;
    table[key] = pos as u32;
    if previous == NO_POSITION as usize
        || previous >= pos
        || pos - previous > params.max_backward_distance
    {
        return find_dictionary_match::<USE_DICTIONARY>(input, pos, max_len, params, best_score);
    }
    if input[previous + best_check] != compare_char {
        return find_dictionary_match::<USE_DICTIONARY>(input, pos, max_len, params, best_score);
    }

    let len = match_len(input, previous, pos, max_len);
    if len < MIN_MATCH_BYTES {
        return find_dictionary_match::<USE_DICTIONARY>(input, pos, max_len, params, best_score);
    }
    let distance = pos - previous;
    let score = score_distance(len, distance);
    if score > best_score {
        return Some(Match {
            len,
            len_code: len,
            distance,
            score,
        });
    }
    find_dictionary_match::<USE_DICTIONARY>(input, pos, max_len, params, best_score)
}

fn find_dictionary_match<const USE_DICTIONARY: bool>(
    input: &[u8],
    pos: usize,
    max_len: usize,
    params: SearchParams,
    min_score: usize,
) -> Option<Match> {
    if USE_DICTIONARY {
        let dictionary_base = params
            .input_base
            .saturating_add(pos)
            .min(params.max_backward_distance);
        find_static_dictionary_identity(input, pos, max_len, dictionary_base, min_score)
    } else {
        None
    }
}

fn find_static_dictionary_identity(
    input: &[u8],
    pos: usize,
    max_len: usize,
    dictionary_base: usize,
    min_score: usize,
) -> Option<Match> {
    let key = (hash14(input, pos) << 1) as usize;
    let item = usize::from(kStaticDictionaryHash[key]);
    if item == 0 {
        return None;
    }

    let len = item & 0x1f;
    let min_len = usize::from(kBrotliMinDictionaryWordLength);
    let max_dict_len = usize::from(kBrotliMaxDictionaryWordLength);
    if len < min_len || len > max_dict_len || len > max_len {
        return None;
    }

    let dist = item >> 5;
    let offset = usize::try_from(kBrotliDictionaryOffsetsByLength[len])
        .ok()?
        .checked_add(len.checked_mul(dist)?)?;
    let word = kBrotliDictionary.get(offset..offset.checked_add(len)?)?;
    let match_len = dictionary_match_len(input, pos, word, len);
    if match_len == 0 || match_len + CUTOFF_TRANSFORMS_COUNT <= len {
        return None;
    }

    let size_bits = usize::from(kBrotliDictionarySizeBitsByLength[len]);
    if dist >= (1_usize << size_bits) {
        return None;
    }
    let cut = len - match_len;
    let transform_id = (cut << 2) + ((CUTOFF_TRANSFORMS >> (cut * 6)) & 0x3f) as usize;
    let distance = dictionary_base
        .checked_add(dist)?
        .checked_add(transform_id.checked_shl(size_bits as u32)?)?
        .checked_add(1)?;
    if distance_code(distance).is_err() {
        return None;
    }
    let score = score_distance(match_len, distance);
    (score >= min_score).then_some(Match {
        len: match_len,
        len_code: len,
        distance,
        score,
    })
}

fn dictionary_match_len(input: &[u8], pos: usize, word: &[u8], max_len: usize) -> usize {
    let Some(candidate) = input.get(pos..pos.saturating_add(max_len)) else {
        return 0;
    };
    let max_len = max_len.min(word.len());
    if max_len == 0 || candidate[0] != word[0] {
        return 0;
    }
    if max_len < 4 {
        let mut len = 1_usize;
        while len < max_len && candidate[len] == word[len] {
            len += 1;
        }
        return len;
    }

    let diff = read_u32_le(candidate, 0) ^ read_u32_le(word, 0);
    if diff != 0 {
        return diff.trailing_zeros() as usize / 8;
    }
    let mut len = 4_usize;
    if max_len >= 8 {
        let diff = read_u32_le(candidate, 4) ^ read_u32_le(word, 4);
        if diff != 0 {
            return 4 + diff.trailing_zeros() as usize / 8;
        }
        len = 8;
    }
    while len + 8 <= max_len {
        let diff = read_u64_le(candidate, len) ^ read_u64_le(word, len);
        if diff != 0 {
            return len + diff.trailing_zeros() as usize / 8;
        }
        len += 8;
    }
    while len < max_len && candidate[len] == word[len] {
        len += 1;
    }
    len
}

fn skip_sparse<const TABLE_BITS_FOR_INPUT: usize>(
    input: &[u8],
    table: &mut [u32],
    mut pos: usize,
    pos_end: usize,
    start: usize,
) -> usize {
    let margin = STORE_LOOKAHEAD - 1;
    if pos > start + 4 * SPARSE_SEARCH_WINDOW {
        let pos_jump = (pos + 16).min(pos_end.saturating_sub(margin));
        while pos < pos_jump {
            let key = table_key::<TABLE_BITS_FOR_INPUT>(input, pos);
            table[key] = pos as u32;
            pos += 4;
        }
    } else {
        let pos_jump = (pos + 8).min(pos_end.saturating_sub(margin));
        while pos < pos_jump {
            let key = table_key::<TABLE_BITS_FOR_INPUT>(input, pos);
            table[key] = pos as u32;
            pos += 2;
        }
    }
    pos
}

fn store_range<const TABLE_BITS_FOR_INPUT: usize, const LONG_MATCH_STORE_STEP: usize>(
    input: &[u8],
    table: &mut [u32],
    start: usize,
    end: usize,
) {
    debug_assert!(LONG_MATCH_STORE_STEP != 0);
    let step = if end.saturating_sub(start) >= LONG_MATCH_STORE_THRESHOLD {
        LONG_MATCH_STORE_STEP
    } else {
        2
    };
    for pos in (start..end).step_by(step) {
        let key = table_key::<TABLE_BITS_FOR_INPUT>(input, pos);
        table[key] = pos as u32;
    }
}

fn table_key<const TABLE_BITS_FOR_INPUT: usize>(input: &[u8], pos: usize) -> usize {
    hash(input, pos, TABLE_BITS_FOR_INPUT)
}

fn table_bits_for_input(input_len: usize) -> usize {
    if input_len <= tune::Q2_TINY_TABLE_MAX_INPUT {
        return 10;
    }
    if input_len <= tune::Q2_SMALL_TABLE_MAX_INPUT {
        12
    } else {
        TABLE_BITS
    }
}

#[inline(always)]
fn hash(input: &[u8], pos: usize, table_bits: usize) -> usize {
    let word = read_u64_le(input, pos) << (64 - 8 * HASH_LEN);
    (word.wrapping_mul(HASH_MUL) >> (64 - table_bits)) as usize
}

fn hash14(input: &[u8], pos: usize) -> u32 {
    read_u32_le(input, pos).wrapping_mul(0x1e35_a7bd) >> (32 - 14)
}
