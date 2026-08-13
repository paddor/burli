use super::{Token, collect_tokens};

pub(super) fn collect(input: &[u8], max_backward_distance: usize) -> Vec<Token> {
    collect_tokens(input, 4, max_backward_distance)
}
