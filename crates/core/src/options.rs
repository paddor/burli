use crate::error::BurliError;
use crate::format::{
    DEFAULT_QUALITY, DEFAULT_WINDOW_BITS, GOOGLE_DEFAULT_QUALITY, MAX_BLOCK_BITS, MAX_QUALITY,
    MAX_WINDOW_BITS, MIN_BLOCK_BITS, MIN_WINDOW_BITS,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct Quality(u8);

impl Quality {
    pub const DEFAULT: Self = Self(DEFAULT_QUALITY);
    pub const GOOGLE_DEFAULT: Self = Self(GOOGLE_DEFAULT_QUALITY);

    pub const fn new(value: u8) -> Result<Self, BurliError> {
        if value <= MAX_QUALITY {
            Ok(Self(value))
        } else {
            Err(BurliError::InvalidQuality(value))
        }
    }

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
pub enum Mode {
    #[default]
    Generic,
    Text,
    Font,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub enum SimdMode {
    #[default]
    Auto,
    Enabled,
    Disabled,
}

#[derive(Clone, Debug, Eq, PartialEq)]
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
    pub fn google_default() -> Self {
        Self {
            quality: Quality::GOOGLE_DEFAULT,
            ..Self::default()
        }
    }

    pub fn quality(mut self, value: u8) -> Result<Self, BurliError> {
        self.quality = Quality::new(value)?;
        Ok(self)
    }

    pub fn quality_value(&self) -> u8 {
        self.quality.get()
    }

    pub fn window_bits(mut self, value: u8) -> Result<Self, BurliError> {
        if !(MIN_WINDOW_BITS..=MAX_WINDOW_BITS).contains(&value) {
            return Err(BurliError::InvalidWindowBits(value));
        }
        self.window_bits = value;
        Ok(self)
    }

    pub const fn window_bits_value(&self) -> u8 {
        self.window_bits
    }

    pub fn block_bits(mut self, value: Option<u8>) -> Result<Self, BurliError> {
        if let Some(bits) = value
            && !(MIN_BLOCK_BITS..=MAX_BLOCK_BITS).contains(&bits)
        {
            return Err(BurliError::InvalidBlockBits(bits));
        }
        self.block_bits = value;
        Ok(self)
    }

    pub const fn block_bits_value(&self) -> Option<u8> {
        self.block_bits
    }

    #[must_use]
    pub const fn mode(mut self, value: Mode) -> Self {
        self.mode = value;
        self
    }

    pub const fn mode_value(&self) -> Mode {
        self.mode
    }

    #[must_use]
    pub const fn simd(mut self, value: SimdMode) -> Self {
        self.simd = value;
        self
    }

    pub const fn simd_value(&self) -> SimdMode {
        self.simd
    }

    #[must_use]
    pub const fn size_hint(mut self, value: Option<usize>) -> Self {
        self.size_hint = value;
        self
    }

    pub const fn size_hint_value(&self) -> Option<usize> {
        self.size_hint
    }

    #[must_use]
    pub const fn disable_literal_context_modeling(mut self, value: bool) -> Self {
        self.disable_literal_context_modeling = value;
        self
    }

    pub const fn literal_context_modeling_disabled(&self) -> bool {
        self.disable_literal_context_modeling
    }
}
