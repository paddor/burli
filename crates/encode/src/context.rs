use alloc::vec::Vec;

use burli_core::{CompressError, Options, bits::BitWriter};

#[derive(Clone, Debug)]
pub struct Compressor {
    options: Options,
    workspace: crate::encode::Workspace,
    writer: BitWriter,
    scratch: Vec<u8>,
}

impl Compressor {
    pub fn new(quality: u8) -> Result<Self, CompressError> {
        Ok(Self {
            options: Options::default().quality(quality)?,
            workspace: crate::encode::Workspace::default(),
            writer: BitWriter::new(),
            scratch: Vec::new(),
        })
    }

    pub fn with_options(options: Options) -> Self {
        Self {
            options,
            workspace: crate::encode::Workspace::default(),
            writer: BitWriter::new(),
            scratch: Vec::new(),
        }
    }

    pub const fn options(&self) -> &Options {
        &self.options
    }

    pub fn reset_options(&mut self, options: Options) {
        self.options = options;
    }

    pub fn compress(&mut self, input: &[u8]) -> Result<Vec<u8>, CompressError> {
        let mut output = Vec::new();
        self.compress_into(input, &mut output)?;
        Ok(output)
    }

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
