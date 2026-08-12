//! Pure Rust Brotli codec.

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

#[cfg(feature = "alloc")]
pub fn compress(input: &[u8], quality: u8) -> Result<alloc::vec::Vec<u8>> {
    burli_encode::compress(input, quality)
}

#[cfg(feature = "alloc")]
pub fn compress_with_options(input: &[u8], options: &Options) -> Result<alloc::vec::Vec<u8>> {
    burli_encode::compress_with_options(input, options)
}

#[cfg(feature = "alloc")]
pub fn compress_into(input: &[u8], output: &mut alloc::vec::Vec<u8>, quality: u8) -> Result<usize> {
    burli_encode::compress_into(input, output, quality)
}

#[cfg(feature = "alloc")]
pub fn compress_into_slice(input: &[u8], output: &mut [u8], quality: u8) -> Result<usize> {
    burli_encode::compress_into_slice(input, output, quality)
}

#[cfg(feature = "alloc")]
pub fn decompress(input: &[u8]) -> Result<alloc::vec::Vec<u8>> {
    burli_decode::decompress(input)
}

#[cfg(feature = "alloc")]
pub fn decompress_with_limit(input: &[u8], max_output_size: usize) -> Result<alloc::vec::Vec<u8>> {
    burli_decode::decompress_with_limit(input, max_output_size)
}

#[cfg(feature = "alloc")]
pub fn decompress_into(input: &[u8], output: &mut alloc::vec::Vec<u8>) -> Result<usize> {
    burli_decode::decompress_into(input, output)
}

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
