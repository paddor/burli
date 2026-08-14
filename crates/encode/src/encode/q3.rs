use alloc::vec::Vec;

use super::{
    INITIAL_LAST_DISTANCE, MIN_MATCH_BYTES, Token, match_len, read_u64_le,
    token_supports_last_distance,
};

const HASH_LEN: usize = 5;
const HASH_TYPE_LEN: usize = 8;
const STORE_LOOKAHEAD: usize = 8;
const HASH_MUL: u64 = 0x1e35_a7bd_1e35_a7bd;
const LITERAL_BYTE_SCORE: usize = 135;
const DISTANCE_BIT_PENALTY: usize = 30;
const SCORE_BASE: usize = DISTANCE_BIT_PENALTY * 8 * core::mem::size_of::<usize>();
const MIN_SCORE: usize = SCORE_BASE + 100;
const LAZY_SCORE_DIFF: usize = 175;
const SPARSE_SEARCH_WINDOW: usize = 64;
const LONG_MATCH_STORE_THRESHOLD: usize = 64;
const TINY_INPUT_THRESHOLD: usize = 1024;
const STACK_TABLE_INPUT_THRESHOLD: usize = 32 * 1024;
const SMALL_MEDIUM_INPUT_THRESHOLD: usize = 320 * 1024;

#[derive(Clone, Copy, Debug)]
struct Match {
    len: usize,
    distance: usize,
    score: usize,
}

#[derive(Clone, Copy, Debug)]
struct SearchParams {
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
    max_backward_distance: usize,
    workspace: &mut Workspace,
) -> Vec<Token> {
    if input.len() <= TINY_INPUT_THRESHOLD {
        collect_with_stack_table_10::<4, 2>(input, max_backward_distance, workspace)
    } else if input.len() <= STACK_TABLE_INPUT_THRESHOLD {
        collect_with_stack_table_15::<1, 2>(input, max_backward_distance, workspace)
    } else if input.len() <= SMALL_MEDIUM_INPUT_THRESHOLD {
        collect_with_params::<15, 1, 2>(input, max_backward_distance, workspace)
    } else {
        collect_with_params::<16, 4, 2>(input, max_backward_distance, workspace)
    }
}

pub(super) fn collect_fast_sweep(
    input: &[u8],
    max_backward_distance: usize,
    workspace: &mut Workspace,
) -> Vec<Token> {
    if input.len() <= TINY_INPUT_THRESHOLD {
        collect_with_stack_table_10::<4, 2>(input, max_backward_distance, workspace)
    } else {
        collect_with_stack_table_15::<1, 1>(input, max_backward_distance, workspace)
    }
}

fn collect_with_params<
    const TABLE_BITS: usize,
    const MAX_LAZY_MATCHES: usize,
    const BUCKET_SWEEP: usize,
>(
    input: &[u8],
    max_backward_distance: usize,
    workspace: &mut Workspace,
) -> Vec<Token> {
    let mut table = vec![0_u32; 1 << TABLE_BITS];
    collect_with_table::<TABLE_BITS, MAX_LAZY_MATCHES, BUCKET_SWEEP>(
        input,
        max_backward_distance,
        workspace,
        &mut table,
    )
}

#[inline(always)]
fn collect_with_stack_table_10<const MAX_LAZY_MATCHES: usize, const BUCKET_SWEEP: usize>(
    input: &[u8],
    max_backward_distance: usize,
    workspace: &mut Workspace,
) -> Vec<Token> {
    let mut table = [0_u32; 1 << 10];
    collect_with_table::<10, MAX_LAZY_MATCHES, BUCKET_SWEEP>(
        input,
        max_backward_distance,
        workspace,
        &mut table,
    )
}

#[allow(clippy::large_stack_arrays)]
#[inline(always)]
fn collect_with_stack_table_15<const MAX_LAZY_MATCHES: usize, const BUCKET_SWEEP: usize>(
    input: &[u8],
    max_backward_distance: usize,
    workspace: &mut Workspace,
) -> Vec<Token> {
    let mut table = [0_u32; 1 << 15];
    collect_with_table::<15, MAX_LAZY_MATCHES, BUCKET_SWEEP>(
        input,
        max_backward_distance,
        workspace,
        &mut table,
    )
}

#[inline(always)]
fn collect_with_table<
    const TABLE_BITS: usize,
    const MAX_LAZY_MATCHES: usize,
    const BUCKET_SWEEP: usize,
>(
    input: &[u8],
    max_backward_distance: usize,
    workspace: &mut Workspace,
    table: &mut [u32],
) -> Vec<Token> {
    debug_assert_eq!(table.len(), 1_usize << TABLE_BITS);
    if input.len() < HASH_TYPE_LEN {
        return literal_only(input.len());
    }

    let mut tokens = Vec::with_capacity(input.len() / 8 + 8);
    let mut pos = 0_usize;
    let mut insert_start = 0_usize;
    let mut dist_cache = workspace.dist_cache;
    let pos_end = input.len();
    let store_end = input.len().saturating_sub(STORE_LOOKAHEAD - 1);
    let mut apply_sparse_search = SPARSE_SEARCH_WINDOW;

    while pos + HASH_TYPE_LEN < pos_end {
        let max_len = pos_end - pos;
        let Some(mut found) = find_match::<TABLE_BITS, BUCKET_SWEEP>(
            input,
            table,
            pos,
            max_len,
            SearchParams {
                max_backward_distance,
                last_distance: dist_cache[0],
                best_len_in: 0,
                min_score: MIN_SCORE,
            },
        ) else {
            pos += 1;
            if pos > apply_sparse_search {
                pos = skip_sparse::<TABLE_BITS, BUCKET_SWEEP>(
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
            if let Some(next) = find_match::<TABLE_BITS, BUCKET_SWEEP>(
                input,
                table,
                lazy_pos,
                lazy_max_len,
                SearchParams {
                    max_backward_distance,
                    last_distance: dist_cache[0],
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
            copy_len_code: 0,
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
        store_range::<TABLE_BITS, BUCKET_SWEEP>(
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

fn find_match<const TABLE_BITS: usize, const BUCKET_SWEEP: usize>(
    input: &[u8],
    table: &mut [u32],
    pos: usize,
    max_len: usize,
    params: SearchParams,
) -> Option<Match> {
    let table_mask = (1 << TABLE_BITS) - 1;
    let key = hash::<TABLE_BITS>(input, pos);
    let sweep_mask = (BUCKET_SWEEP - 1) << 3;
    let key_out = (key + (pos & sweep_mask)) & table_mask;
    let best_check = params.best_len_in.min(max_len.saturating_sub(1));
    let mut compare_char = input[pos + best_check];
    let mut best_score = params.min_score;
    let mut best_len = best_check;
    let mut out = None;

    if pos >= params.last_distance {
        let previous = pos - params.last_distance;
        if input[previous + best_check] == compare_char {
            let len = match_len(input, previous, pos, max_len);
            if len >= MIN_MATCH_BYTES {
                let score = score_last_distance(len);
                if score > best_score {
                    best_len = len;
                    best_score = score;
                    if len < max_len {
                        compare_char = input[pos + len];
                    }
                    out = Some(Match {
                        len,
                        distance: params.last_distance,
                        score,
                    });
                }
            }
        }
    }

    for slot in 0..BUCKET_SWEEP {
        if best_len >= max_len {
            break;
        }
        let entry = table[(key + (slot << 3)) & table_mask] as usize;
        if entry == 0 {
            continue;
        }
        let previous = entry - 1;
        if previous >= pos || pos - previous > params.max_backward_distance {
            continue;
        }
        if input[previous + best_len] != compare_char {
            continue;
        }

        let len = match_len(input, previous, pos, max_len);
        if len < MIN_MATCH_BYTES {
            continue;
        }
        let distance = pos - previous;
        let score = score_distance(len, distance);
        if score > best_score {
            best_len = len;
            best_score = score;
            if len < max_len {
                compare_char = input[pos + len];
            }
            out = Some(Match {
                len,
                distance,
                score,
            });
        }
    }

    table[key_out] = (pos + 1) as u32;
    out
}

fn skip_sparse<const TABLE_BITS: usize, const BUCKET_SWEEP: usize>(
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
            store::<TABLE_BITS, BUCKET_SWEEP>(input, table, pos);
            pos += 4;
        }
    } else {
        let pos_jump = (pos + 8).min(pos_end.saturating_sub(margin));
        while pos < pos_jump {
            store::<TABLE_BITS, BUCKET_SWEEP>(input, table, pos);
            pos += 2;
        }
    }
    pos
}

fn store_range<const TABLE_BITS: usize, const BUCKET_SWEEP: usize>(
    input: &[u8],
    table: &mut [u32],
    start: usize,
    end: usize,
) {
    let step = if end.saturating_sub(start) >= LONG_MATCH_STORE_THRESHOLD {
        4
    } else {
        2
    };
    for pos in (start..end).step_by(step) {
        store::<TABLE_BITS, BUCKET_SWEEP>(input, table, pos);
    }
}

fn store<const TABLE_BITS: usize, const BUCKET_SWEEP: usize>(
    input: &[u8],
    table: &mut [u32],
    pos: usize,
) {
    let table_mask = (1 << TABLE_BITS) - 1;
    let sweep_mask = (BUCKET_SWEEP - 1) << 3;
    let key = (hash::<TABLE_BITS>(input, pos) + (pos & sweep_mask)) & table_mask;
    table[key] = (pos + 1) as u32;
}

#[inline(always)]
fn hash<const TABLE_BITS: usize>(input: &[u8], pos: usize) -> usize {
    let word = read_u64_le(input, pos) << (64 - 8 * HASH_LEN);
    (word.wrapping_mul(HASH_MUL) >> (64 - TABLE_BITS)) as usize
}

fn score_distance(len: usize, distance: usize) -> usize {
    SCORE_BASE + LITERAL_BYTE_SCORE * len
        - DISTANCE_BIT_PENALTY * (usize::BITS as usize - 1 - distance.leading_zeros() as usize)
}

fn score_last_distance(len: usize) -> usize {
    SCORE_BASE + LITERAL_BYTE_SCORE * len + 15
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
