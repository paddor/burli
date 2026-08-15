use alloc::vec::Vec;

use burli_core::{DecompressError, format::DEFAULT_MAX_OUTPUT_SIZE};

#[derive(Clone, Debug)]
/// Reusable one-shot Brotli decompressor.
///
/// The decompressor keeps scratch output capacity across
/// [`decompress_into`](Self::decompress_into) and
/// [`decompress_into_slice`](Self::decompress_into_slice) calls.
pub struct Decompressor {
    max_output_size: usize,
    scratch: Vec<u8>,
}

impl Decompressor {
    /// Create a decompressor with no practical output limit.
    pub const fn new() -> Self {
        Self {
            max_output_size: DEFAULT_MAX_OUTPUT_SIZE,
            scratch: Vec::new(),
        }
    }

    /// Create a decompressor with a hard output limit.
    pub fn with_limit(max_output_size: usize) -> Self {
        Self {
            max_output_size,
            scratch: Vec::new(),
        }
    }

    /// Return the configured maximum output size.
    pub const fn max_output_size(&self) -> usize {
        self.max_output_size
    }

    /// Replace the output limit without releasing reusable buffers.
    pub fn set_limit(&mut self, max_output_size: usize) {
        self.max_output_size = max_output_size;
    }

    /// Decompress `input` into a new `Vec`.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed streams or output-limit violations.
    pub fn decompress(&mut self, input: &[u8]) -> Result<Vec<u8>, DecompressError> {
        crate::decompress_with_limit(input, self.max_output_size)
    }

    /// Decompress `input` and append to `output`.
    ///
    /// Returns the number of bytes appended. The caller buffer is not modified
    /// on decode errors.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed streams or output-limit violations.
    pub fn decompress_into(
        &mut self,
        input: &[u8],
        output: &mut Vec<u8>,
    ) -> Result<usize, DecompressError> {
        let before = output.len();
        self.scratch.clear();
        crate::stored::decompress_into_empty_with_limit(
            input,
            self.max_output_size,
            &mut self.scratch,
        )?;
        output.extend_from_slice(&self.scratch);
        Ok(output.len() - before)
    }

    /// Decompress `input` into a caller-provided slice.
    ///
    /// Returns the number of bytes written. The slice is not partially written
    /// on size errors.
    ///
    /// # Errors
    ///
    /// Returns [`DecompressError::OutputLimitExceeded`] when `output` is too
    /// small.
    pub fn decompress_into_slice(
        &mut self,
        input: &[u8],
        output: &mut [u8],
    ) -> Result<usize, DecompressError> {
        let limit = self.max_output_size.min(output.len());
        self.scratch.clear();
        crate::stored::decompress_into_empty_with_limit(input, limit, &mut self.scratch)?;
        output[..self.scratch.len()].copy_from_slice(&self.scratch);
        Ok(self.scratch.len())
    }
}

impl Default for Decompressor {
    fn default() -> Self {
        Self::new()
    }
}

/// Backward-compatible alias for [`Decompressor`].
pub struct DecompressContext {
    inner: Decompressor,
}

impl DecompressContext {
    /// Create a context with no practical output limit.
    pub const fn new() -> Self {
        Self {
            inner: Decompressor::new(),
        }
    }

    /// Create a context with a hard output limit.
    pub fn with_limit(max_output_size: usize) -> Self {
        Self {
            inner: Decompressor::with_limit(max_output_size),
        }
    }

    /// Return the configured maximum output size.
    pub const fn max_output_size(&self) -> usize {
        self.inner.max_output_size()
    }

    /// Replace the output limit without releasing reusable buffers.
    pub fn set_limit(&mut self, max_output_size: usize) {
        self.inner.set_limit(max_output_size);
    }

    /// Decompress `input` into a new `Vec`.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed streams or output-limit violations.
    pub fn decompress(&mut self, input: &[u8]) -> Result<Vec<u8>, DecompressError> {
        self.inner.decompress(input)
    }

    /// Decompress `input` and append to `output`.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed streams or output-limit violations.
    pub fn decompress_into(
        &mut self,
        input: &[u8],
        output: &mut Vec<u8>,
    ) -> Result<usize, DecompressError> {
        self.inner.decompress_into(input, output)
    }

    /// Decompress `input` into a caller-provided slice.
    ///
    /// # Errors
    ///
    /// Returns [`DecompressError::OutputLimitExceeded`] when `output` is too
    /// small.
    pub fn decompress_into_slice(
        &mut self,
        input: &[u8],
        output: &mut [u8],
    ) -> Result<usize, DecompressError> {
        self.inner.decompress_into_slice(input, output)
    }
}

impl Default for DecompressContext {
    fn default() -> Self {
        Self::new()
    }
}
