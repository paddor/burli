pub(super) const MAX_DELAYED_SYMBOLS: usize = 0x2fff;
pub(super) const Q1_DELAYED_SYMBOLS: usize = 0x9fff;
pub(super) const Q4_DELAYED_SYMBOLS: usize = 3840;
pub(super) const Q5_DELAYED_SYMBOLS: usize = 3584;
pub(super) const LOW_COMPRESS_DELAYED_SYMBOLS: usize = MAX_DELAYED_SYMBOLS;

pub(super) const Q0_ONE_PASS_MAX_INPUT: usize = 64 * 1024;
pub(super) const Q1_TWO_PASS_MAX_INPUT: usize = 64 * 1024;
/// Skip start of the q1 matcher up to [`Q1_TWO_PASS_MAX_INPUT`]: after 32
/// misses it looks at every other position, as in Google's encoder.
pub(super) const Q1_TWO_PASS_SKIP_START: usize = 32;
/// Above [`Q1_TWO_PASS_MAX_INPUT`], q1 matches 6 bytes and more in text,
/// with a table of this many positions, and puts the last `Q1_LONG_TAIL`
/// positions of each copy into it. Binary input takes the
/// `Q1_LONG_BINARY_*` values. On Silesia, more positions or tail entries
/// compress text past q2, and 5-byte matches lose up to 13% on text.
pub(super) const Q1_LONG_TABLE_SIZE: usize = 1 << 16;
pub(super) const Q1_LONG_MIN_MATCH: usize = 6;
pub(super) const Q1_LONG_TAIL: usize = 4;
pub(super) const Q1_LONG_BINARY_TABLE_SIZE: usize = 1 << 15;
pub(super) const Q1_LONG_BINARY_MIN_MATCH: usize = 5;
pub(super) const Q1_LONG_BINARY_TAIL: usize = 5;
/// Copies of q1 above [`Q1_TWO_PASS_MAX_INPUT`] reach back at most this
/// far, as in Google's encoder.
pub(super) const Q1_LONG_MAX_DISTANCE: usize = (1 << 18) - 16;
pub(super) const Q2_STATIC_NO_DICTIONARY_MAX_INPUT: usize = 4 * 1024;
pub(super) const Q2_MEDIUM_H3_MIN_INPUT: usize = 8 * 1024;
pub(super) const Q2_MEDIUM_H3_MAX_INPUT: usize = 128 * 1024;
pub(super) const Q2_FAST_H3_MAX_INPUT: usize = 16 * 1024;
pub(super) const Q2_SWEEP1_H3_MAX_INPUT: usize = 128 * 1024;
pub(super) const Q3_FAST_SWEEP_MAX_INPUT: usize = 16 * 1024;
pub(super) const Q3_MEDIUM_SWEEP1_MIN_INPUT: usize = 144 * 1024;
pub(super) const Q3_MEDIUM_SWEEP1_MAX_INPUT: usize = 160 * 1024;
pub(super) const Q4_TINY_CONTEXT_MAX_INPUT: usize = 768;

pub(super) const LOW_COMPRESS_SAMPLE_MIN_INPUT: usize = 64 * 1024;
pub(super) const LOW_COMPRESS_SAMPLE_BYTES: usize = 64 * 1024;
pub(super) const LOW_COMPRESS_SAMPLE_STEP: usize = 64;
pub(super) const LOW_COMPRESS_SAMPLE_COUNT: usize =
    LOW_COMPRESS_SAMPLE_BYTES / LOW_COMPRESS_SAMPLE_STEP;
pub(super) const LOW_COMPRESS_SMALL_SAMPLES: usize = 256;
pub(super) const LOW_COMPRESS_SMALL_MIN_STEP: usize = 4;
pub(super) const LOW_COMPRESS_SMALL_TABLE_SIZE: usize = 1 << 10;
pub(super) const LOW_COMPRESS_TEXT_PREFIX_SAMPLES: usize = 32;
pub(super) const LOW_COMPRESS_TEXT_PREFIX_MIN: usize = 31;
/// Per quality, low-compressibility blocks get a literal-only Huffman code
/// from this size up and are stored below it. q0 always stores them. Higher
/// qualities code shorter blocks. Measured on Silesia `x-ray`, the literal
/// code writes about 590 MB/s at 32 KiB, 475 MB/s at 16 KiB, 225 MB/s at
/// 4 KiB, 80 MB/s at 1 KiB, and 47 MB/s at 512 B.
pub(super) const LOW_COMPRESS_LITERAL_MIN_INPUT: [usize; 6] =
    [usize::MAX, 32 * 1024, 16 * 1024, 4 * 1024, 1024, 512];
/// Longer low-compressibility blocks split into literal-only meta-blocks of
/// about this size, each with its own code. On Silesia `x-ray` this gives
/// ratio 1.249 instead of 1.208 for one 8 MiB block, 2% slower.
pub(super) const LOW_COMPRESS_LITERAL_BLOCK_SIZE: usize = 256 * 1024;
/// From this size up, low-compressibility blocks at q1 and up get literal
/// codes by the context of the previous byte, in meta-blocks of about
/// `LOW_COMPRESS_CONTEXT_BLOCK_SIZE`, with at most this many codes per
/// quality. Merges of codes look at neighbors until at most
/// `LOW_COMPRESS_CONTEXT_ALL_PAIRS` codes remain, then at all pairs.
pub(super) const LOW_COMPRESS_CONTEXT_MIN_INPUT: usize = 64 * 1024;
pub(super) const LOW_COMPRESS_CONTEXT_BLOCK_SIZE: usize = 1 << 20;
pub(super) const LOW_COMPRESS_CONTEXT_CODES: [usize; 6] = [1, 2, 3, 4, 16, 64];
pub(super) const LOW_COMPRESS_CONTEXT_ALL_PAIRS: [usize; 6] = [0, 0, 0, 0, 24, 32];
/// Per quality, the counts take the first this many bytes of every
/// `context_literals::SAMPLE_PERIOD`, or all of them at that size. Fewer
/// counts make q1 faster than q2.
pub(super) const LOW_COMPRESS_CONTEXT_SAMPLE: [usize; 6] = [1024, 128, 1024, 1024, 1024, 1024];
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

pub(super) const Q1_DEFAULT_U32_SKIP_START: usize = 32;
pub(super) const Q2_USE_DICTIONARY_MAX_INPUT: usize = 128 * 1024;
pub(super) const Q2_TINY_PREFIX_MIN_INPUT: usize = 385;
pub(super) const Q2_TINY_PREFIX_MAX_INPUT: usize = 1024;
pub(super) const Q2_TINY_TABLE_MAX_INPUT: usize = 1024;
pub(super) const Q2_SMALL_TABLE_MAX_INPUT: usize = 8 * 1024;

pub(super) const Q3_TINY_INPUT_MAX: usize = 1024;
pub(super) const Q3_SMALL_MEDIUM_INPUT_MAX: usize = 320 * 1024;

pub(super) const Q4_TINY_INPUT_MAX: usize = 1024;
pub(super) const Q4_SMALL_MEDIUM_INPUT_MAX: usize = 320 * 1024;
pub(super) const Q4_LARGE_INPUT_MIN: usize = 1 << 20;

pub(super) const Q5_TINY_INPUT_MAX: usize = 1024;
pub(super) const Q5_SMALL_INPUT_MAX: usize = 96 * 1024;
pub(super) const Q5_MEDIUM_INPUT_MAX: usize = 160 * 1024;
pub(super) const Q5_SMALL_MEDIUM_INPUT_MAX: usize = 320 * 1024;
pub(super) const Q5_LARGE_INPUT_MIN: usize = 1 << 20;
