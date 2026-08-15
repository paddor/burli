use core::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
/// Error type used by public Bürli APIs.
pub enum BurliError {
    /// Encoder quality was outside Brotli's 0 through 11 range.
    InvalidQuality(u8),
    /// Window size was outside Brotli's standard range.
    InvalidWindowBits(u8),
    /// Meta-block size override was outside Brotli's standard range.
    InvalidBlockBits(u8),
    /// The output buffer or configured output limit was too small.
    OutputLimitExceeded { limit: usize, needed: usize },
    /// The requested feature is not implemented.
    Unsupported(&'static str),
    /// The input stream or requested output format was invalid.
    Format(&'static str),
}

/// Encoder error type.
pub type CompressError = BurliError;
/// Decoder error type.
pub type DecompressError = BurliError;
/// Result type used by public Bürli APIs.
pub type Result<T> = core::result::Result<T, BurliError>;

impl fmt::Display for BurliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidQuality(value) => write!(f, "invalid Brotli quality: {value}"),
            Self::InvalidWindowBits(value) => write!(f, "invalid Brotli window bits: {value}"),
            Self::InvalidBlockBits(value) => write!(f, "invalid Brotli block bits: {value}"),
            Self::OutputLimitExceeded { limit, needed } => {
                write!(f, "output limit exceeded: limit={limit}, needed={needed}")
            }
            Self::Unsupported(message) | Self::Format(message) => f.write_str(message),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for BurliError {}
