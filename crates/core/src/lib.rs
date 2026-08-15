//! Shared Brotli types, errors, bit primitives, and format constants.

#![cfg_attr(not(feature = "std"), no_std)]
#![deny(unsafe_op_in_unsafe_fn)]
#![cfg_attr(feature = "paranoid", forbid(unsafe_code))]

#[cfg(feature = "alloc")]
extern crate alloc;

pub mod error;
pub mod format;
pub mod options;
pub mod simd;

pub mod bits;
#[doc(hidden)]
pub mod dictionary;

pub use error::{BurliError, CompressError, DecompressError, Result};
pub use options::{Mode, Options, Quality, SimdMode};
