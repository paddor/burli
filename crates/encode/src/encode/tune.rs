pub(super) const MAX_DELAYED_SYMBOLS: usize = 0x2fff;
pub(super) const Q1_DELAYED_SYMBOLS: usize = 0x9fff;
pub(super) const Q4_DELAYED_SYMBOLS: usize = 3840;
pub(super) const Q5_DELAYED_SYMBOLS: usize = 3584;
pub(super) const LOW_COMPRESS_DELAYED_SYMBOLS: usize = MAX_DELAYED_SYMBOLS;

pub(super) const Q0_DIRECT_MAX_INPUT: usize = 384;
pub(super) const Q0_STATIC_ENTROPY_MAX_INPUT: usize = 1024;
pub(super) const Q0_COLLECT_FAST_NO_LAST_MAX_INPUT: usize = 2 * 1024;
pub(super) const Q0_COLLECT_NO_LAST_MAX_INPUT: usize = 4 * 1024;
pub(super) const Q0_COLLECT_DEFAULT_MAX_INPUT: usize = 8 * 1024;
pub(super) const Q0_COLLECT_MEDIUM_NO_LAST_MAX_INPUT: usize = 16 * 1024;
pub(super) const Q0_COLLECT_MEDIUM_MAX_INPUT: usize = 32 * 1024;
pub(super) const Q0_COLLECT_SAMPLED_MAX_INPUT: usize = 128 * 1024;
pub(super) const Q0_COLLECT_HUGE_MIN_INPUT: usize = 1024 * 1024 + 1;
pub(super) const Q0_WRITE_BALANCED_LITERAL_MAX_INPUT: usize = 2 * 1024;
pub(super) const Q0_WRITE_PACKED_LITERAL_MAX_INPUT: usize = 8 * 1024;
pub(super) const Q0_WRITE_FAST_COMMAND_MAX_INPUT: usize = 16 * 1024;
pub(super) const Q0_WRITE_SAMPLED_MAX_INPUT: usize = 128 * 1024;
pub(super) const Q1_STATIC_ENTROPY_MAX_INPUT: usize = 1024;
pub(super) const Q1_LONG_INPUT_MIN: usize = 128 * 1024 + 1;
pub(super) const Q2_STATIC_NO_DICTIONARY_MAX_INPUT: usize = 4 * 1024;
pub(super) const Q2_MEDIUM_H3_MIN_INPUT: usize = 8 * 1024;
pub(super) const Q2_MEDIUM_H3_MAX_INPUT: usize = 128 * 1024;
pub(super) const Q2_FAST_H3_MAX_INPUT: usize = 16 * 1024;
pub(super) const Q2_SWEEP1_H3_MAX_INPUT: usize = 128 * 1024;
pub(super) const Q3_FAST_SWEEP_MAX_INPUT: usize = 16 * 1024;
pub(super) const Q3_MEDIUM_SWEEP1_MIN_INPUT: usize = 144 * 1024;
pub(super) const Q3_MEDIUM_SWEEP1_MAX_INPUT: usize = 160 * 1024;
pub(super) const Q4_TINY_CONTEXT_MAX_INPUT: usize = 768;

pub(super) const Q0_DENSE_DUP6_MIN: usize = 512;
pub(super) const Q0_LOW_DUP6_MAX: usize = 199;

pub(super) const LOW_COMPRESS_SAMPLE_MIN_INPUT: usize = 64 * 1024;
pub(super) const LOW_COMPRESS_SAMPLE_BYTES: usize = 64 * 1024;
pub(super) const LOW_COMPRESS_SAMPLE_STEP: usize = 64;
pub(super) const LOW_COMPRESS_DUP6_STORE_MAX: usize = 45;
pub(super) const LOW_COMPRESS_ZERO_RATIO_NUM: usize = 1;
pub(super) const LOW_COMPRESS_ZERO_RATIO_DEN: usize = 50;
pub(super) const LOW_COMPRESS_PRINTABLE_RATIO_NUM: usize = 4;
pub(super) const LOW_COMPRESS_PRINTABLE_RATIO_DEN: usize = 5;

pub(super) const Q0_LOW_COMPRESS_STORE_BLOCK_BITS: usize = 18;
pub(super) const Q0_LOW_COMPRESS_STORE_BLOCK_MASK: usize = 15;
pub(super) const Q0_LOW_COMPRESS_STORE_BLOCKS: u16 = !((1 << 0) | (1 << 5) | (1 << 10));

pub(super) const Q1_LOW_COMPRESS_BLOCK_SIZE: usize = 512 * 1024;
pub(super) const Q1_LOW_COMPRESS_STORE_BLOCK_MASK: usize = 1;
pub(super) const Q1_LOW_COMPRESS_STORE_BLOCKS: u16 = 1 << 1;
pub(super) const Q2_LOW_COMPRESS_SPARSE_STRIDE: usize = 512;
pub(super) const Q3_LOW_COMPRESS_SPARSE_STRIDE: usize = 128;
pub(super) const Q4_LOW_COMPRESS_SPARSE_STRIDE: usize = 6;
pub(super) const Q5_LOW_COMPRESS_SPARSE_STRIDE: usize = 1;

pub(super) const Q1_DEFAULT_U16_SKIP_START: usize = 61;
pub(super) const Q1_MEDIUM_U16_SKIP_START: usize = 72;
pub(super) const Q1_FAST_U16_SKIP_START: usize = 96;
pub(super) const Q1_FASTER_U16_SKIP_START: usize = 128;
pub(super) const Q1_DEFAULT_U32_SKIP_START: usize = 32;
pub(super) const Q1_MEDIUM_U32_SKIP_START: usize = 64;
pub(super) const Q1_DENSE_U32_SKIP_START: usize = 80;
pub(super) const Q1_FAST_U32_SKIP_START: usize = 96;
pub(super) const Q1_FASTER_U32_SKIP_START: usize = 128;

pub(super) const Q1_LARGE_MARKUP_SAMPLE_BYTES: usize = 1024;
pub(super) const Q1_LARGE_MARKUP_MIN_LT: usize = 8;
pub(super) const Q1_LARGE_MARKUP_MIN_GT: usize = 8;
pub(super) const Q1_CONTENT_SAMPLE_BYTES: usize = 64 * 1024;
pub(super) const Q1_TABULAR_PRINTABLE_PCT: usize = 98;
pub(super) const Q1_TABULAR_WHITESPACE_PCT: usize = 40;
pub(super) const Q1_TABULAR_ALPHA_MAX_PCT: usize = 20;
pub(super) const Q1_ZERO_HIGH_ZERO_PCT: usize = 25;
pub(super) const Q1_ZERO_HIGH_HIGH_PCT: usize = 2;
pub(super) const Q1_FAST_WRITER_WHITESPACE_PCT: usize = 18;
pub(super) const Q1_FAST_WRITER_TEXT_PRINTABLE_PCT: usize = 85;
pub(super) const Q1_FAST_WRITER_TEXT_ALPHA_PCT: usize = 50;
pub(super) const Q1_FAST_WRITER_TEXT_WHITESPACE_PCT: usize = 10;
pub(super) const Q1_FAST_WRITER_TEXT_ANGLE_MAX_PCT: usize = 5;

pub(super) const Q2_USE_DICTIONARY_MAX_INPUT: usize = 128 * 1024;
pub(super) const Q2_TINY_PREFIX_MIN_INPUT: usize = 385;
pub(super) const Q2_TINY_PREFIX_MAX_INPUT: usize = 1024;
pub(super) const Q2_TINY_TABLE_MAX_INPUT: usize = 1024;
pub(super) const Q2_SMALL_TABLE_MAX_INPUT: usize = 8 * 1024;

pub(super) const Q3_TINY_INPUT_MAX: usize = 1024;
pub(super) const Q3_SMALL_MEDIUM_INPUT_MAX: usize = 320 * 1024;
pub(super) const Q3_STACK_TABLE_INPUT_MAX: usize = Q3_SMALL_MEDIUM_INPUT_MAX;

pub(super) const Q4_TINY_INPUT_MAX: usize = 1024;
pub(super) const Q4_SMALL_MEDIUM_INPUT_MAX: usize = 320 * 1024;
pub(super) const Q4_LARGE_INPUT_MIN: usize = 1 << 20;

pub(super) const Q5_TINY_INPUT_MAX: usize = 1024;
pub(super) const Q5_SMALL_INPUT_MAX: usize = 96 * 1024;
pub(super) const Q5_MEDIUM_INPUT_MAX: usize = 160 * 1024;
pub(super) const Q5_SMALL_MEDIUM_INPUT_MAX: usize = 320 * 1024;
pub(super) const Q5_LARGE_INPUT_MIN: usize = 1 << 20;
