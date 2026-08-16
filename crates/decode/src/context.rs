use alloc::vec::Vec;

use burli_core::{DecompressError, format::DEFAULT_MAX_OUTPUT_SIZE};

use crate::Options;

#[derive(Clone, Debug)]
/// Reusable one-shot Brotli decompressor.
///
/// The decompressor keeps scratch output capacity across
/// [`decompress_into`](Self::decompress_into) and
/// [`decompress_into_slice`](Self::decompress_into_slice) calls.
pub struct Decompressor {
    max_output_size: usize,
    raw_dictionary: Vec<u8>,
    scratch: Vec<u8>,
}

impl Decompressor {
    /// Create a decompressor with no practical output limit.
    pub const fn new() -> Self {
        Self {
            max_output_size: DEFAULT_MAX_OUTPUT_SIZE,
            raw_dictionary: Vec::new(),
            scratch: Vec::new(),
        }
    }

    /// Create a decompressor with explicit [`Options`].
    pub fn with_options(options: &Options) -> Self {
        Self {
            max_output_size: options.max_output_size_value(),
            raw_dictionary: Vec::new(),
            scratch: Vec::new(),
        }
    }

    /// Create a decompressor with a hard output limit.
    pub fn with_limit(max_output_size: usize) -> Self {
        Self::with_options(&Options::new().max_output_size(max_output_size))
    }

    /// Create a decompressor with a raw LZ77 prefix dictionary.
    pub fn with_raw_dictionary(dictionary: &[u8]) -> Self {
        Self {
            max_output_size: DEFAULT_MAX_OUTPUT_SIZE,
            raw_dictionary: dictionary.to_vec(),
            scratch: Vec::new(),
        }
    }

    /// Create a decompressor with a raw LZ77 prefix dictionary and hard output
    /// limit.
    pub fn with_raw_dictionary_and_limit(dictionary: &[u8], max_output_size: usize) -> Self {
        Self {
            max_output_size,
            raw_dictionary: dictionary.to_vec(),
            scratch: Vec::new(),
        }
    }

    /// Return current decode options.
    pub fn options(&self) -> Options {
        Options::new().max_output_size(self.max_output_size)
    }

    /// Replace decode options without releasing reusable buffers or changing
    /// the dictionary.
    pub fn reset_options(&mut self, options: &Options) {
        self.max_output_size = options.max_output_size_value();
    }

    /// Return the configured maximum output size.
    pub const fn max_output_size(&self) -> usize {
        self.max_output_size
    }

    /// Replace the output limit without releasing reusable buffers.
    pub fn set_limit(&mut self, max_output_size: usize) {
        self.max_output_size = max_output_size;
    }

    /// Replace the raw LZ77 prefix dictionary.
    pub fn set_raw_dictionary(&mut self, dictionary: &[u8]) {
        self.raw_dictionary.clear();
        self.raw_dictionary.extend_from_slice(dictionary);
    }

    /// Remove the raw LZ77 prefix dictionary.
    pub fn clear_raw_dictionary(&mut self) {
        self.raw_dictionary.clear();
    }

    /// Decompress `input` into a new `Vec`.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed streams or output-limit violations.
    pub fn decompress(&mut self, input: &[u8]) -> Result<Vec<u8>, DecompressError> {
        crate::stored::decompress_with_raw_dictionary_and_limit(
            input,
            crate::dictionary::RawDictionary::new(&self.raw_dictionary),
            self.max_output_size,
        )
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
            crate::dictionary::RawDictionary::new(&self.raw_dictionary),
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
        crate::stored::decompress_into_empty_with_limit(
            input,
            limit,
            &mut self.scratch,
            crate::dictionary::RawDictionary::new(&self.raw_dictionary),
        )?;
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

    /// Create a context with explicit [`Options`].
    pub fn with_options(options: &Options) -> Self {
        Self {
            inner: Decompressor::with_options(options),
        }
    }

    /// Create a context with a hard output limit.
    pub fn with_limit(max_output_size: usize) -> Self {
        Self {
            inner: Decompressor::with_limit(max_output_size),
        }
    }

    /// Create a context with a raw LZ77 prefix dictionary.
    pub fn with_raw_dictionary(dictionary: &[u8]) -> Self {
        Self {
            inner: Decompressor::with_raw_dictionary(dictionary),
        }
    }

    /// Create a context with a raw LZ77 prefix dictionary and hard output
    /// limit.
    pub fn with_raw_dictionary_and_limit(dictionary: &[u8], max_output_size: usize) -> Self {
        Self {
            inner: Decompressor::with_raw_dictionary_and_limit(dictionary, max_output_size),
        }
    }

    /// Return current decode options.
    pub fn options(&self) -> Options {
        self.inner.options()
    }

    /// Replace decode options without releasing reusable buffers or changing
    /// the dictionary.
    pub fn reset_options(&mut self, options: &Options) {
        self.inner.reset_options(options);
    }

    /// Return the configured maximum output size.
    pub const fn max_output_size(&self) -> usize {
        self.inner.max_output_size()
    }

    /// Replace the output limit without releasing reusable buffers.
    pub fn set_limit(&mut self, max_output_size: usize) {
        self.inner.set_limit(max_output_size);
    }

    /// Replace the raw LZ77 prefix dictionary.
    pub fn set_raw_dictionary(&mut self, dictionary: &[u8]) {
        self.inner.set_raw_dictionary(dictionary);
    }

    /// Remove the raw LZ77 prefix dictionary.
    pub fn clear_raw_dictionary(&mut self) {
        self.inner.clear_raw_dictionary();
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
