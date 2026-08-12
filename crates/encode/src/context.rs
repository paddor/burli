use burli_core::{CompressError, Options};

pub struct CompressContext {
    options: Options,
}

impl CompressContext {
    pub fn new(options: Options) -> Self {
        Self { options }
    }

    pub fn with_quality(quality: u8) -> Result<Self, CompressError> {
        Ok(Self {
            options: Options::default().quality(quality)?,
        })
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
}
