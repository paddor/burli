mod header;
pub(crate) mod uncompressed;

pub(crate) use header::{write_last_empty_meta_block, write_window_bits};
pub(crate) use uncompressed::{compress_uncompressed_with_options, write_uncompressed_meta_block};
