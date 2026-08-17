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

mod options;

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
pub use options::Options;
#[cfg(feature = "alloc")]
pub use options::RawDictionary;

/// Decompress a complete Brotli stream.
///
/// # Errors
///
/// Returns an error for malformed streams, unsupported large-window streams, or
/// output-limit violations.
#[cfg(feature = "alloc")]
pub fn decompress(input: &[u8]) -> Result<alloc::vec::Vec<u8>, DecompressError> {
    decompress_with_options(input, &Options::new())
}

/// Decompress a complete Brotli stream with explicit [`Options`].
///
/// # Errors
///
/// Returns an error for malformed streams, unsupported large-window streams, or
/// output-limit violations.
#[cfg(feature = "alloc")]
pub fn decompress_with_options(
    input: &[u8],
    options: &Options,
) -> Result<alloc::vec::Vec<u8>, DecompressError> {
    stored::decompress_with_raw_dictionary_and_limit(
        input,
        crate::dictionary::RawDictionary::empty(),
        options.max_output_size(),
    )
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
    decompress_with_options(input, &Options::new().with_max_output_size(max_output_size))
}

#[doc(hidden)]
#[cfg(feature = "alloc")]
pub fn decompress_concat_payload_with_limit(
    input: &[u8],
    payload_bit_len: usize,
    window_bits: u8,
    max_output_size: usize,
) -> Result<(alloc::vec::Vec<u8>, bool), DecompressError> {
    stored::decompress_concat_payload_with_limit(
        input,
        payload_bit_len,
        window_bits,
        max_output_size,
    )
}

/// Decompress a complete Brotli stream with a raw LZ77 prefix dictionary.
///
/// The dictionary must match the dictionary used by the encoder. This does not
/// parse serialized shared dictionaries and does not replace Brotli's static
/// dictionary.
///
/// # Errors
///
/// Returns an error for malformed streams, unsupported large-window streams, or
/// output-limit violations.
#[cfg(feature = "alloc")]
pub fn decompress_with_raw_dictionary(
    input: &[u8],
    dictionary: &RawDictionary,
) -> Result<alloc::vec::Vec<u8>, DecompressError> {
    decompress_with_raw_dictionary_and_options(input, dictionary, &Options::new())
}

/// Decompress a complete Brotli stream with a raw LZ77 prefix dictionary and
/// explicit [`Options`].
///
/// # Errors
///
/// Returns an error for malformed streams, unsupported large-window streams, or
/// output-limit violations.
#[cfg(feature = "alloc")]
pub fn decompress_with_raw_dictionary_and_options(
    input: &[u8],
    dictionary: &RawDictionary,
    options: &Options,
) -> Result<alloc::vec::Vec<u8>, DecompressError> {
    stored::decompress_with_raw_dictionary_and_limit(
        input,
        crate::dictionary::RawDictionary::new(dictionary.as_bytes()),
        options.max_output_size(),
    )
}

/// Decompress a complete Brotli stream with a raw LZ77 prefix dictionary and
/// maximum output size.
///
/// # Errors
///
/// Returns [`DecompressError::OutputLimitExceeded`] if the decoded stream would
/// exceed `max_output_size`.
#[cfg(feature = "alloc")]
pub fn decompress_with_raw_dictionary_and_limit(
    input: &[u8],
    dictionary: &RawDictionary,
    max_output_size: usize,
) -> Result<alloc::vec::Vec<u8>, DecompressError> {
    decompress_with_raw_dictionary_and_options(
        input,
        dictionary,
        &Options::new().with_max_output_size(max_output_size),
    )
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
    decompress_into_with_options(input, output, &Options::new())
}

/// Decompress `input` and append bytes to `output` with explicit [`Options`].
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
    options: &Options,
) -> Result<usize, DecompressError> {
    let before = output.len();
    let mut decompressed = alloc::vec::Vec::new();
    stored::decompress_into_empty_with_limit(
        input,
        options.max_output_size(),
        &mut decompressed,
        crate::dictionary::RawDictionary::empty(),
    )?;
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
    decompress_into_slice_with_options(input, output, &Options::new())
}

/// Decompress `input` into a caller-provided slice with explicit [`Options`].
///
/// Returns the number of bytes written. The slice is not partially written on
/// size errors.
///
/// # Errors
///
/// Returns [`DecompressError::OutputLimitExceeded`] if `output` is too small or
/// if the configured output limit is exceeded.
#[cfg(feature = "alloc")]
pub fn decompress_into_slice_with_options(
    input: &[u8],
    output: &mut [u8],
    options: &Options,
) -> Result<usize, DecompressError> {
    let limit = options.max_output_size().min(output.len());
    let mut decompressed = alloc::vec::Vec::with_capacity(output.len());
    stored::decompress_into_empty_with_limit(
        input,
        limit,
        &mut decompressed,
        crate::dictionary::RawDictionary::empty(),
    )?;
    output[..decompressed.len()].copy_from_slice(&decompressed);
    Ok(decompressed.len())
}
