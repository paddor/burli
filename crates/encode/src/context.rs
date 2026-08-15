use alloc::vec::Vec;

use burli_core::{CompressError, Options, bits::BitWriter};

#[derive(Clone, Debug)]
/// Reusable one-shot Brotli compressor.
///
/// The compressor keeps match-finder tables, bit-writer capacity, and scratch
/// buffers across calls. Prefer [`compress_into`](Self::compress_into) or
/// [`compress_into_slice`](Self::compress_into_slice) in hot loops.
pub struct Compressor {
    options: Options,
    workspace: crate::encode::Workspace,
    writer: BitWriter,
    scratch: Vec<u8>,
}

impl Compressor {
    /// Create a compressor for `quality`.
    ///
    /// # Errors
    ///
    /// Returns [`CompressError::InvalidQuality`] outside Brotli's quality range.
    pub fn new(quality: u8) -> Result<Self, CompressError> {
        Ok(Self {
            options: Options::default().quality(quality)?,
            workspace: crate::encode::Workspace::default(),
            writer: BitWriter::new(),
            scratch: Vec::new(),
        })
    }

    /// Create a compressor with explicit [`Options`].
    pub fn with_options(options: Options) -> Self {
        Self {
            options,
            workspace: crate::encode::Workspace::default(),
            writer: BitWriter::new(),
            scratch: Vec::new(),
        }
    }

    /// Return current options.
    pub const fn options(&self) -> &Options {
        &self.options
    }

    /// Replace options without releasing reusable buffers.
    pub fn reset_options(&mut self, options: Options) {
        self.options = options;
    }

    /// Compress `input` into a new `Vec`.
    ///
    /// # Errors
    ///
    /// Returns an error when the current options are unsupported.
    pub fn compress(&mut self, input: &[u8]) -> Result<Vec<u8>, CompressError> {
        let mut output = Vec::new();
        self.compress_into(input, &mut output)?;
        Ok(output)
    }

    /// Compress `input` and append to `output`.
    ///
    /// Returns the number of bytes appended.
    ///
    /// # Errors
    ///
    /// Returns an error when the current options are unsupported.
    pub fn compress_into(
        &mut self,
        input: &[u8],
        output: &mut Vec<u8>,
    ) -> Result<usize, CompressError> {
        crate::encode::compress_into_with_options_workspace(
            input,
            &self.options,
            &mut self.workspace,
            &mut self.writer,
            output,
        )
    }

    /// Compress `input` into a caller-provided slice.
    ///
    /// Returns the number of bytes written. The slice is not partially written
    /// on size errors.
    ///
    /// # Errors
    ///
    /// Returns [`CompressError::OutputLimitExceeded`] when `output` is too
    /// small.
    pub fn compress_into_slice(
        &mut self,
        input: &[u8],
        output: &mut [u8],
    ) -> Result<usize, CompressError> {
        self.scratch.clear();
        crate::encode::compress_into_with_options_workspace(
            input,
            &self.options,
            &mut self.workspace,
            &mut self.writer,
            &mut self.scratch,
        )?;
        if self.scratch.len() > output.len() {
            return Err(CompressError::OutputLimitExceeded {
                limit: output.len(),
                needed: self.scratch.len(),
            });
        }
        output[..self.scratch.len()].copy_from_slice(&self.scratch);
        Ok(self.scratch.len())
    }
}

/// Backward-compatible alias for [`Compressor`].
pub struct CompressContext {
    inner: Compressor,
}

impl CompressContext {
    /// Create a context from explicit [`Options`].
    pub fn new(options: Options) -> Self {
        Self {
            inner: Compressor::with_options(options),
        }
    }

    /// Create a context for `quality`.
    ///
    /// # Errors
    ///
    /// Returns [`CompressError::InvalidQuality`] outside Brotli's quality range.
    pub fn with_quality(quality: u8) -> Result<Self, CompressError> {
        Ok(Self {
            inner: Compressor::new(quality)?,
        })
    }

    /// Return current options.
    pub const fn options(&self) -> &Options {
        self.inner.options()
    }

    /// Replace options without releasing reusable buffers.
    pub fn reset_options(&mut self, options: Options) {
        self.inner.reset_options(options);
    }

    /// Compress `input` into a new `Vec`.
    ///
    /// # Errors
    ///
    /// Returns an error when the current options are unsupported.
    pub fn compress(&mut self, input: &[u8]) -> Result<Vec<u8>, CompressError> {
        self.inner.compress(input)
    }

    /// Compress `input` and append to `output`.
    ///
    /// # Errors
    ///
    /// Returns an error when the current options are unsupported.
    pub fn compress_into(
        &mut self,
        input: &[u8],
        output: &mut Vec<u8>,
    ) -> Result<usize, CompressError> {
        self.inner.compress_into(input, output)
    }

    /// Compress `input` into a caller-provided slice.
    ///
    /// # Errors
    ///
    /// Returns [`CompressError::OutputLimitExceeded`] when `output` is too
    /// small.
    pub fn compress_into_slice(
        &mut self,
        input: &[u8],
        output: &mut [u8],
    ) -> Result<usize, CompressError> {
        self.inner.compress_into_slice(input, output)
    }
}
