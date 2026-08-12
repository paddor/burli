use burli_core::{DecompressError, format::DEFAULT_MAX_OUTPUT_SIZE};

pub struct DecompressContext {
    max_output_size: usize,
}

impl DecompressContext {
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
}

impl Default for DecompressContext {
    fn default() -> Self {
        Self::new()
    }
}
