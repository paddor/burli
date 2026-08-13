use burli_core::CompressError;

use super::{PreparedBatch, collect_prepared_tokens_q0};

pub(super) fn collect(
    input: &[u8],
    max_backward_distance: usize,
) -> Result<PreparedBatch, CompressError> {
    collect_prepared_tokens_q0(input, max_backward_distance)
}
