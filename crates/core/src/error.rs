use core::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BurliError {
    InvalidQuality(u8),
    InvalidWindowBits(u8),
    InvalidBlockBits(u8),
    OutputLimitExceeded { limit: usize, needed: usize },
    Unsupported(&'static str),
    Format(&'static str),
}

pub type CompressError = BurliError;
pub type DecompressError = BurliError;
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
