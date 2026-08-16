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
pub use burli_encode as encode;

/// Decode-specific API surface.
pub mod decode {
    pub use burli_decode::Options;
    #[cfg(feature = "alloc")]
    pub use burli_decode::RawDictionary;
}

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
/// Use [`decompress_with_options`] for untrusted input with a hard output cap.
///
/// # Errors
///
/// Returns an error for malformed streams, unsupported large-window streams, or
/// output-limit violations.
#[cfg(feature = "alloc")]
pub fn decompress(input: &[u8]) -> Result<alloc::vec::Vec<u8>> {
    burli_decode::decompress(input)
}

/// Decompress a complete Brotli stream with explicit [`decode::Options`].
///
/// # Errors
///
/// Returns an error for malformed streams, unsupported large-window streams, or
/// output-limit violations.
#[cfg(feature = "alloc")]
pub fn decompress_with_options(
    input: &[u8],
    options: &decode::Options,
) -> Result<alloc::vec::Vec<u8>> {
    burli_decode::decompress_with_options(input, options)
}

/// Decompress a complete Brotli stream with a maximum output size.
///
/// # Errors
///
/// Returns [`BurliError::OutputLimitExceeded`] if the decoded stream would
/// exceed `max_output_size`.
#[cfg(feature = "alloc")]
pub fn decompress_with_limit(input: &[u8], max_output_size: usize) -> Result<alloc::vec::Vec<u8>> {
    decompress_with_options(
        input,
        &decode::Options::new().max_output_size(max_output_size),
    )
}

/// Decompress a complete Brotli stream with a raw LZ77 prefix dictionary.
///
/// # Errors
///
/// Returns an error for malformed streams or output-limit violations.
#[cfg(feature = "alloc")]
pub fn decompress_with_raw_dictionary(
    input: &[u8],
    dictionary: &decode::RawDictionary,
) -> Result<alloc::vec::Vec<u8>> {
    burli_decode::decompress_with_raw_dictionary(input, dictionary)
}

/// Decompress a complete Brotli stream with a raw LZ77 prefix dictionary and
/// explicit [`decode::Options`].
///
/// # Errors
///
/// Returns an error for malformed streams, unsupported large-window streams, or
/// output-limit violations.
#[cfg(feature = "alloc")]
pub fn decompress_with_raw_dictionary_and_options(
    input: &[u8],
    dictionary: &decode::RawDictionary,
    options: &decode::Options,
) -> Result<alloc::vec::Vec<u8>> {
    burli_decode::decompress_with_raw_dictionary_and_options(input, dictionary, options)
}

/// Decompress a complete Brotli stream with a raw LZ77 prefix dictionary and
/// maximum output size.
///
/// # Errors
///
/// Returns [`BurliError::OutputLimitExceeded`] if the decoded stream would
/// exceed `max_output_size`.
#[cfg(feature = "alloc")]
pub fn decompress_with_raw_dictionary_and_limit(
    input: &[u8],
    dictionary: &decode::RawDictionary,
    max_output_size: usize,
) -> Result<alloc::vec::Vec<u8>> {
    decompress_with_raw_dictionary_and_options(
        input,
        dictionary,
        &decode::Options::new().max_output_size(max_output_size),
    )
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
    decompress_into_with_options(input, output, &decode::Options::new())
}

/// Decompress `input` and append bytes to `output` with explicit
/// [`decode::Options`].
///
/// Returns the number of bytes appended. The caller buffer is not modified when
/// decoding fails.
///
/// # Errors
///
/// Returns an error for malformed streams or output-limit violations.
#[cfg(feature = "alloc")]
pub fn decompress_into_with_options(
    input: &[u8],
    output: &mut alloc::vec::Vec<u8>,
    options: &decode::Options,
) -> Result<usize> {
    burli_decode::decompress_into_with_options(input, output, options)
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
    decompress_into_slice_with_options(input, output, &decode::Options::new())
}

/// Decompress `input` into a caller-provided slice with explicit
/// [`decode::Options`].
///
/// Returns the number of bytes written. The slice is not partially written on
/// size errors.
///
/// # Errors
///
/// Returns [`BurliError::OutputLimitExceeded`] if `output` is too small or if
/// the configured output limit is exceeded.
#[cfg(feature = "alloc")]
pub fn decompress_into_slice_with_options(
    input: &[u8],
    output: &mut [u8],
    options: &decode::Options,
) -> Result<usize> {
    burli_decode::decompress_into_slice_with_options(input, output, options)
}

#[cfg(feature = "alloc")]
pub use burli_decode::context::{DecompressContext, Decompressor};
#[cfg(feature = "std")]
pub use burli_decode::streaming::StreamDecoder;
#[cfg(feature = "alloc")]
pub use burli_encode::context::{CompressContext, Compressor};
#[cfg(feature = "std")]
pub use burli_encode::streaming::StreamEncoder;
