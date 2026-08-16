use crate::error::BurliError;
use crate::format::{
    DEFAULT_QUALITY, DEFAULT_WINDOW_BITS, GOOGLE_DEFAULT_QUALITY, MAX_BLOCK_BITS, MAX_QUALITY,
    MAX_WINDOW_BITS, MIN_BLOCK_BITS, MIN_WINDOW_BITS,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
/// Brotli encoder quality level.
///
/// Bürli accepts the standard Brotli range at the type level. The encoder
/// currently implements qualities 0 through 5.
pub struct Quality(u8);

impl Quality {
    /// Bürli's default quality.
    pub const DEFAULT: Self = Self(DEFAULT_QUALITY);
    /// Google Brotli's default quality.
    pub const GOOGLE_DEFAULT: Self = Self(GOOGLE_DEFAULT_QUALITY);

    /// Build a checked quality value.
    ///
    /// # Errors
    ///
    /// Returns [`BurliError::InvalidQuality`] when `value` is greater than 11.
    pub const fn new(value: u8) -> Result<Self, BurliError> {
        if value <= MAX_QUALITY {
            Ok(Self(value))
        } else {
            Err(BurliError::InvalidQuality(value))
        }
    }

    /// Return the raw quality level.
    pub const fn get(self) -> u8 {
        self.0
    }
}

impl Default for Quality {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl TryFrom<u8> for Quality {
    type Error = BurliError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
/// Brotli input mode hint.
pub enum Mode {
    /// Generic binary or mixed input.
    #[default]
    Generic,
    /// Text input.
    Text,
    /// Font input.
    Font,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
/// SIMD dispatch policy.
pub enum SimdMode {
    /// Use runtime dispatch when available.
    #[default]
    Auto,
    /// Prefer SIMD paths when available.
    Enabled,
    /// Force scalar paths.
    Disabled,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// Brotli encode options.
pub struct Options {
    quality: Quality,
    window_bits: u8,
    block_bits: Option<u8>,
    mode: Mode,
    simd: SimdMode,
    size_hint: Option<usize>,
    disable_literal_context_modeling: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            quality: Quality::DEFAULT,
            window_bits: DEFAULT_WINDOW_BITS,
            block_bits: None,
            mode: Mode::Generic,
            simd: SimdMode::Auto,
            size_hint: None,
            disable_literal_context_modeling: false,
        }
    }
}

impl Options {
    /// Build options matching Google Brotli's default quality.
    pub fn google_default() -> Self {
        Self {
            quality: Quality::GOOGLE_DEFAULT,
            ..Self::default()
        }
    }

    /// Set the encoder quality.
    ///
    /// # Errors
    ///
    /// Returns [`BurliError::InvalidQuality`] for values outside 0 through 11.
    pub fn with_quality(mut self, value: u8) -> Result<Self, BurliError> {
        self.quality = Quality::new(value)?;
        Ok(self)
    }

    /// Return the raw quality value.
    pub fn quality(&self) -> u8 {
        self.quality.get()
    }

    /// Set the Brotli window size as log2 bytes.
    ///
    /// # Errors
    ///
    /// Returns [`BurliError::InvalidWindowBits`] outside the standard range.
    pub fn with_window_bits(mut self, value: u8) -> Result<Self, BurliError> {
        if !(MIN_WINDOW_BITS..=MAX_WINDOW_BITS).contains(&value) {
            return Err(BurliError::InvalidWindowBits(value));
        }
        self.window_bits = value;
        Ok(self)
    }

    /// Return the configured window size as log2 bytes.
    pub const fn window_bits(&self) -> u8 {
        self.window_bits
    }

    /// Set the target meta-block size as log2 bytes.
    ///
    /// `None` lets the encoder choose a quality-specific default.
    ///
    /// # Errors
    ///
    /// Returns [`BurliError::InvalidBlockBits`] outside the standard range.
    pub fn with_block_bits(mut self, value: Option<u8>) -> Result<Self, BurliError> {
        if let Some(bits) = value
            && !(MIN_BLOCK_BITS..=MAX_BLOCK_BITS).contains(&bits)
        {
            return Err(BurliError::InvalidBlockBits(bits));
        }
        self.block_bits = value;
        Ok(self)
    }

    /// Return the optional block-size override.
    pub const fn block_bits(&self) -> Option<u8> {
        self.block_bits
    }

    /// Set the input mode hint.
    #[must_use]
    pub const fn with_mode(mut self, value: Mode) -> Self {
        self.mode = value;
        self
    }

    /// Return the input mode hint.
    pub const fn mode(&self) -> Mode {
        self.mode
    }

    /// Set the SIMD dispatch policy.
    #[must_use]
    pub const fn with_simd(mut self, value: SimdMode) -> Self {
        self.simd = value;
        self
    }

    /// Return the SIMD dispatch policy.
    pub const fn simd(&self) -> SimdMode {
        self.simd
    }

    /// Set an input-size hint for callers that cannot pass the full slice yet.
    #[must_use]
    pub const fn with_size_hint(mut self, value: Option<usize>) -> Self {
        self.size_hint = value;
        self
    }

    /// Return the optional input-size hint.
    pub const fn size_hint(&self) -> Option<usize> {
        self.size_hint
    }

    /// Disable literal context modeling.
    ///
    /// This can trade ratio for speed at higher qualities.
    #[must_use]
    pub const fn with_literal_context_modeling_disabled(mut self, value: bool) -> Self {
        self.disable_literal_context_modeling = value;
        self
    }

    /// Return true when literal context modeling is disabled.
    pub const fn literal_context_modeling_disabled(&self) -> bool {
        self.disable_literal_context_modeling
    }
}
