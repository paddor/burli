#![cfg_attr(not(feature = "std"), no_std)]
#![deny(unsafe_op_in_unsafe_fn)]
#![cfg_attr(feature = "paranoid", forbid(unsafe_code))]

#[cfg(feature = "alloc")]
extern crate alloc;

#[cfg(feature = "std")]
pub mod context;
#[cfg(feature = "std")]
pub mod streaming;

pub use burli_core::{DecompressError, format::DEFAULT_MAX_OUTPUT_SIZE};

#[cfg(feature = "alloc")]
pub fn decompress(input: &[u8]) -> Result<alloc::vec::Vec<u8>, DecompressError> {
    decompress_with_limit(input, DEFAULT_MAX_OUTPUT_SIZE)
}

#[cfg(feature = "alloc")]
pub fn decompress_with_limit(
    _input: &[u8],
    _max_output_size: usize,
) -> Result<alloc::vec::Vec<u8>, DecompressError> {
    Err(DecompressError::Unsupported(
        "burli decoder not implemented yet",
    ))
}

#[cfg(feature = "alloc")]
pub fn decompress_into(
    input: &[u8],
    output: &mut alloc::vec::Vec<u8>,
) -> Result<usize, DecompressError> {
    let before = output.len();
    let decompressed = decompress(input)?;
    output.extend_from_slice(&decompressed);
    Ok(output.len() - before)
}
