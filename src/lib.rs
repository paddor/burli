//! Pure Rust Brotli codec.
//!
//! The crate exposes small one-shot helpers first, reusable contexts for
//! repeated work, and `std::io` streaming types behind the `std` feature.
//! Encoding currently supports qualities 0 through 5. Decoding accepts standard
//! Brotli streams produced at all normal quality levels.

#![cfg_attr(not(feature = "std"), no_std)]
#![deny(unsafe_op_in_unsafe_fn)]
#![cfg_attr(feature = "paranoid", forbid(unsafe_code))]

#[cfg(feature = "alloc")]
extern crate alloc;

pub use burli_core::error;
pub use burli_core::format;
pub use burli_core::{
    BurliError, CompressError, DecompressError, Mode, Options, Quality, Result, SimdMode,
};

#[doc(hidden)]
pub use burli_decode as decode;
#[doc(hidden)]
pub use burli_encode as encode;

/// Compress `input` at Brotli `quality`.
///
/// Use [`Compressor`] when compressing many independent inputs with the same
/// options.
///
/// # Errors
///
/// Returns an error for invalid quality values or unsupported encoder options.
#[cfg(feature = "alloc")]
pub fn compress(input: &[u8], quality: u8) -> Result<alloc::vec::Vec<u8>> {
    burli_encode::compress(input, quality)
}

/// Compress `input` with explicit [`Options`].
///
/// # Errors
///
/// Returns an error when the options are outside the implemented encoder scope.
#[cfg(feature = "alloc")]
pub fn compress_with_options(input: &[u8], options: &Options) -> Result<alloc::vec::Vec<u8>> {
    burli_encode::compress_with_options(input, options)
}

/// Compress `input` and append the Brotli stream to `output`.
///
/// Returns the number of bytes appended.
///
/// # Errors
///
/// Returns an error for invalid quality values or unsupported encoder options.
#[cfg(feature = "alloc")]
pub fn compress_into(input: &[u8], output: &mut alloc::vec::Vec<u8>, quality: u8) -> Result<usize> {
    burli_encode::compress_into(input, output, quality)
}

/// Compress `input` into a caller-provided slice.
///
/// Returns the number of bytes written. The slice is not partially written on
/// size errors.
///
/// # Errors
///
/// Returns [`BurliError::OutputLimitExceeded`] if `output` is too small.
#[cfg(feature = "alloc")]
pub fn compress_into_slice(input: &[u8], output: &mut [u8], quality: u8) -> Result<usize> {
    burli_encode::compress_into_slice(input, output, quality)
}

/// Decompress a complete Brotli stream.
///
/// Use [`decompress_with_limit`] for untrusted input with a hard output cap.
///
/// # Errors
///
/// Returns an error for malformed streams, unsupported large-window streams, or
/// output-limit violations.
#[cfg(feature = "alloc")]
pub fn decompress(input: &[u8]) -> Result<alloc::vec::Vec<u8>> {
    burli_decode::decompress(input)
}

/// Decompress a complete Brotli stream with a maximum output size.
///
/// # Errors
///
/// Returns [`BurliError::OutputLimitExceeded`] if the decoded stream would
/// exceed `max_output_size`.
#[cfg(feature = "alloc")]
pub fn decompress_with_limit(input: &[u8], max_output_size: usize) -> Result<alloc::vec::Vec<u8>> {
    burli_decode::decompress_with_limit(input, max_output_size)
}

/// Decompress `input` and append bytes to `output`.
///
/// Returns the number of bytes appended. The caller buffer is not modified when
/// decoding fails.
///
/// # Errors
///
/// Returns an error for malformed streams or output-limit violations.
#[cfg(feature = "alloc")]
pub fn decompress_into(input: &[u8], output: &mut alloc::vec::Vec<u8>) -> Result<usize> {
    burli_decode::decompress_into(input, output)
}

/// Decompress `input` into a caller-provided slice.
///
/// Returns the number of bytes written. The slice is not partially written on
/// size errors.
///
/// # Errors
///
/// Returns [`BurliError::OutputLimitExceeded`] if `output` is too small.
#[cfg(feature = "alloc")]
pub fn decompress_into_slice(input: &[u8], output: &mut [u8]) -> Result<usize> {
    burli_decode::decompress_into_slice(input, output)
}

#[cfg(feature = "alloc")]
pub use burli_decode::context::{DecompressContext, Decompressor};
#[cfg(feature = "std")]
pub use burli_decode::streaming::StreamDecoder;
#[cfg(feature = "alloc")]
pub use burli_encode::context::{CompressContext, Compressor};
#[cfg(feature = "std")]
pub use burli_encode::streaming::StreamEncoder;
