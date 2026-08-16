#[cfg(feature = "alloc")]
use alloc::vec::Vec;

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

/// Raw LZ77 prefix dictionary for Brotli decode APIs.
///
/// The dictionary must match the dictionary used by the encoder. This does not
/// parse serialized shared dictionaries and does not replace Brotli's static
/// dictionary.
#[cfg(feature = "alloc")]
#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct RawDictionary {
    bytes: Vec<u8>,
}

#[cfg(feature = "alloc")]
impl RawDictionary {
    /// Build an owned raw dictionary from bytes.
    #[must_use]
    pub fn new(bytes: &[u8]) -> Self {
        Self {
            bytes: bytes.to_vec(),
        }
    }

    /// Build an empty raw dictionary.
    #[must_use]
    pub const fn empty() -> Self {
        Self { bytes: Vec::new() }
    }

    /// Build an owned raw dictionary from an existing vector.
    #[must_use]
    pub const fn from_vec(bytes: Vec<u8>) -> Self {
        Self { bytes }
    }

    /// Return dictionary bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Return dictionary byte length.
    #[must_use]
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    /// Return true when no dictionary bytes are configured.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }
}

#[cfg(feature = "alloc")]
impl From<Vec<u8>> for RawDictionary {
    fn from(bytes: Vec<u8>) -> Self {
        Self::from_vec(bytes)
    }
}

#[cfg(feature = "alloc")]
impl From<&[u8]> for RawDictionary {
    fn from(bytes: &[u8]) -> Self {
        Self::new(bytes)
    }
}

#[cfg(feature = "alloc")]
impl AsRef<[u8]> for RawDictionary {
    fn as_ref(&self) -> &[u8] {
        self.as_bytes()
    }
}
