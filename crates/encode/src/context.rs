use alloc::vec::Vec;

use burli_core::{CompressError, Options};

#[derive(Clone, Debug)]
pub struct Compressor {
    options: Options,
}

impl Compressor {
    pub fn new(quality: u8) -> Result<Self, CompressError> {
        Ok(Self {
            options: Options::default().quality(quality)?,
        })
    }

    pub fn with_options(options: Options) -> Self {
        Self { options }
    }

    pub const fn options(&self) -> &Options {
        &self.options
    }

    pub fn reset_options(&mut self, options: Options) {
        self.options = options;
    }

    pub fn compress(&mut self, input: &[u8]) -> Result<Vec<u8>, CompressError> {
        crate::compress_with_options(input, &self.options)
    }

    pub fn compress_into(
        &mut self,
        input: &[u8],
        output: &mut Vec<u8>,
    ) -> Result<usize, CompressError> {
        let before = output.len();
        let compressed = self.compress(input)?;
        output.extend_from_slice(&compressed);
        Ok(output.len() - before)
    }

    pub fn compress_into_slice(
        &mut self,
        input: &[u8],
        output: &mut [u8],
    ) -> Result<usize, CompressError> {
        let compressed = self.compress(input)?;
        if compressed.len() > output.len() {
            return Err(CompressError::OutputLimitExceeded {
                limit: output.len(),
                needed: compressed.len(),
            });
        }
        output[..compressed.len()].copy_from_slice(&compressed);
        Ok(compressed.len())
    }
}

pub struct CompressContext {
    inner: Compressor,
}

impl CompressContext {
    pub fn new(options: Options) -> Self {
        Self {
            inner: Compressor::with_options(options),
        }
    }

    pub fn with_quality(quality: u8) -> Result<Self, CompressError> {
        Ok(Self {
            inner: Compressor::new(quality)?,
        })
    }

    pub const fn options(&self) -> &Options {
        self.inner.options()
    }

    pub fn reset_options(&mut self, options: Options) {
        self.inner.reset_options(options);
    }

    pub fn compress(&mut self, input: &[u8]) -> Result<Vec<u8>, CompressError> {
        self.inner.compress(input)
    }

    pub fn compress_into(
        &mut self,
        input: &[u8],
        output: &mut Vec<u8>,
    ) -> Result<usize, CompressError> {
        self.inner.compress_into(input, output)
    }

    pub fn compress_into_slice(
        &mut self,
        input: &[u8],
        output: &mut [u8],
    ) -> Result<usize, CompressError> {
        self.inner.compress_into_slice(input, output)
    }
}
