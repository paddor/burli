use burli_core::format::DEFAULT_MAX_OUTPUT_SIZE;

/// Brotli decode options.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Options {
    max_output_size: usize,
}

impl Options {
    /// Build options with no practical output limit.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            max_output_size: DEFAULT_MAX_OUTPUT_SIZE,
        }
    }

    /// Set the maximum decoded output size.
    #[must_use]
    pub const fn max_output_size(mut self, limit: usize) -> Self {
        self.max_output_size = limit;
        self
    }

    /// Return the maximum decoded output size.
    #[must_use]
    pub const fn max_output_size_value(&self) -> usize {
        self.max_output_size
    }
}

impl Default for Options {
    fn default() -> Self {
        Self::new()
    }
}
