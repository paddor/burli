use alloc::vec::Vec;

use burli_core::{DecompressError, format::DEFAULT_MAX_OUTPUT_SIZE};

#[derive(Clone, Debug)]
pub struct Decompressor {
    max_output_size: usize,
}

impl Decompressor {
    pub const fn new() -> Self {
        Self {
            max_output_size: DEFAULT_MAX_OUTPUT_SIZE,
        }
    }

    pub const fn with_limit(max_output_size: usize) -> Self {
        Self { max_output_size }
    }

    pub const fn max_output_size(&self) -> usize {
        self.max_output_size
    }

    pub fn set_limit(&mut self, max_output_size: usize) {
        self.max_output_size = max_output_size;
    }

    pub fn decompress(&mut self, input: &[u8]) -> Result<Vec<u8>, DecompressError> {
        crate::decompress_with_limit(input, self.max_output_size)
    }

    pub fn decompress_into(
        &mut self,
        input: &[u8],
        output: &mut Vec<u8>,
    ) -> Result<usize, DecompressError> {
        let before = output.len();
        let decompressed = self.decompress(input)?;
        output.extend_from_slice(&decompressed);
        Ok(output.len() - before)
    }
}

impl Default for Decompressor {
    fn default() -> Self {
        Self::new()
    }
}

pub struct DecompressContext {
    inner: Decompressor,
}

impl DecompressContext {
    pub const fn new() -> Self {
        Self {
            inner: Decompressor::new(),
        }
    }

    pub const fn with_limit(max_output_size: usize) -> Self {
        Self {
            inner: Decompressor::with_limit(max_output_size),
        }
    }

    pub const fn max_output_size(&self) -> usize {
        self.inner.max_output_size()
    }

    pub fn set_limit(&mut self, max_output_size: usize) {
        self.inner.set_limit(max_output_size);
    }

    pub fn decompress(&mut self, input: &[u8]) -> Result<Vec<u8>, DecompressError> {
        self.inner.decompress(input)
    }

    pub fn decompress_into(
        &mut self,
        input: &[u8],
        output: &mut Vec<u8>,
    ) -> Result<usize, DecompressError> {
        self.inner.decompress_into(input, output)
    }
}

impl Default for DecompressContext {
    fn default() -> Self {
        Self::new()
    }
}
