#![cfg_attr(not(feature = "std"), no_std)]
#![deny(unsafe_op_in_unsafe_fn)]
#![cfg_attr(feature = "paranoid", forbid(unsafe_code))]

#[cfg(feature = "alloc")]
extern crate alloc;

#[cfg(feature = "alloc")]
pub mod context;
#[cfg(feature = "alloc")]
#[doc(hidden)]
pub mod diagnostics;
#[cfg(feature = "std")]
pub mod streaming;

#[cfg(feature = "alloc")]
mod encode;
#[cfg(feature = "alloc")]
mod metablock;

pub use burli_core::{CompressError, Options};

#[cfg(feature = "alloc")]
pub fn compress(input: &[u8], quality: u8) -> Result<alloc::vec::Vec<u8>, CompressError> {
    let options = Options::default().quality(quality)?;
    compress_with_options(input, &options)
}

#[cfg(feature = "alloc")]
pub fn compress_with_options(
    input: &[u8],
    options: &Options,
) -> Result<alloc::vec::Vec<u8>, CompressError> {
    encode::compress_with_options(input, options)
}

#[cfg(feature = "alloc")]
pub fn compress_into(
    input: &[u8],
    output: &mut alloc::vec::Vec<u8>,
    quality: u8,
) -> Result<usize, CompressError> {
    let options = Options::default().quality(quality)?;
    let mut workspace = encode::Workspace::default();
    let mut writer = burli_core::bits::BitWriter::new();
    encode::compress_into_with_options_workspace(
        input,
        &options,
        &mut workspace,
        &mut writer,
        output,
    )
}

#[cfg(feature = "alloc")]
pub fn compress_into_slice(
    input: &[u8],
    output: &mut [u8],
    quality: u8,
) -> Result<usize, CompressError> {
    let compressed = compress(input, quality)?;
    if compressed.len() > output.len() {
        return Err(CompressError::OutputLimitExceeded {
            limit: output.len(),
            needed: compressed.len(),
        });
    }
    output[..compressed.len()].copy_from_slice(&compressed);
    Ok(compressed.len())
}
