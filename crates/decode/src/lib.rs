//! Brotli decoder implementation for Bürli.

#![cfg_attr(not(feature = "std"), no_std)]
#![deny(unsafe_op_in_unsafe_fn)]
#![cfg_attr(feature = "paranoid", forbid(unsafe_code))]

#[cfg(feature = "alloc")]
extern crate alloc;

#[cfg(feature = "alloc")]
pub mod context;
#[cfg(feature = "std")]
pub mod streaming;

#[cfg(feature = "alloc")]
mod compressed;
#[cfg(feature = "alloc")]
mod context_lookup;
#[cfg(feature = "alloc")]
mod dictionary;
#[cfg(feature = "alloc")]
mod huffman;
#[cfg(feature = "alloc")]
mod stored;

pub use burli_core::{DecompressError, format::DEFAULT_MAX_OUTPUT_SIZE};

/// Decompress a complete Brotli stream.
///
/// # Errors
///
/// Returns an error for malformed streams, unsupported large-window streams, or
/// output-limit violations.
#[cfg(feature = "alloc")]
pub fn decompress(input: &[u8]) -> Result<alloc::vec::Vec<u8>, DecompressError> {
    decompress_with_limit(input, DEFAULT_MAX_OUTPUT_SIZE)
}

/// Decompress a complete Brotli stream with a maximum output size.
///
/// # Errors
///
/// Returns [`DecompressError::OutputLimitExceeded`] if the decoded stream would
/// exceed `max_output_size`.
#[cfg(feature = "alloc")]
pub fn decompress_with_limit(
    input: &[u8],
    max_output_size: usize,
) -> Result<alloc::vec::Vec<u8>, DecompressError> {
    stored::decompress_with_limit(input, max_output_size)
}

/// Decompress `input` and append bytes to `output`.
///
/// # Errors
///
/// Returns an error for malformed streams or output-limit violations.
#[cfg(feature = "alloc")]
pub fn decompress_into(
    input: &[u8],
    output: &mut alloc::vec::Vec<u8>,
) -> Result<usize, DecompressError> {
    let before = output.len();
    let mut decompressed = alloc::vec::Vec::new();
    stored::decompress_into_empty_with_limit(input, DEFAULT_MAX_OUTPUT_SIZE, &mut decompressed)?;
    output.extend_from_slice(&decompressed);
    Ok(output.len() - before)
}

/// Decompress `input` into a caller-provided slice.
///
/// # Errors
///
/// Returns [`DecompressError::OutputLimitExceeded`] if `output` is too small.
#[cfg(feature = "alloc")]
pub fn decompress_into_slice(input: &[u8], output: &mut [u8]) -> Result<usize, DecompressError> {
    let mut decompressed = alloc::vec::Vec::with_capacity(output.len());
    stored::decompress_into_empty_with_limit(input, output.len(), &mut decompressed)?;
    output[..decompressed.len()].copy_from_slice(&decompressed);
    Ok(decompressed.len())
}
