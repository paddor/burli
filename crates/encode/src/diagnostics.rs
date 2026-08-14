use burli_core::{CompressError, Options};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Q0StoreStats {
    pub input_bytes: usize,
    pub blocks: usize,
    pub sampled_blocks: usize,
    pub store_candidate_blocks: usize,
    pub stored_blocks: usize,
    pub stored_bytes: usize,
    pub sampled_positions: usize,
    pub sampled_load_bytes: usize,
    pub duplicate_6_count: usize,
    pub sampled_match_bytes: usize,
    pub zero_count: usize,
    pub printable_count: usize,
    pub max_sample_miss_streak: usize,
    pub skipped_probe_positions: usize,
}

pub fn q0_store_stats(input: &[u8], options: &Options) -> Result<Q0StoreStats, CompressError> {
    crate::encode::q0_store_stats(input, options)
}
