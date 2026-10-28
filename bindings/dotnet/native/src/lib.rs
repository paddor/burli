//! Native library of the .NET binding for burli. Only `Native.cs` in this
//! package calls it; the functions are no public C API.
//!
//! Status codes are the values of .NET `OperationStatus`. Every function
//! catches panics, so none unwinds into .NET. Handles come from the create
//! functions, and .NET `SafeHandle`s destroy each one once, after its last
//! call.

use std::io::Write;
use std::panic::{self, AssertUnwindSafe};
use std::slice;

use burli::{BurliError, DecodeStatus, Options, PushDecoder, StreamEncoder};

const DONE: i32 = 0;
const DESTINATION_TOO_SMALL: i32 = 1;
const NEED_MORE_DATA: i32 = 2;
const INVALID_DATA: i32 = 3;

/// `BrotliEncoderOperation` values.
const OPERATION_FLUSH: i32 = 1;
const OPERATION_FINISH: i32 = 2;

/// The highest quality the encoder implements. Higher ones run as this.
const MAX_QUALITY: i32 = 5;

/// Runs `work`, turning a panic into `INVALID_DATA`.
fn guard(work: impl FnOnce() -> i32) -> i32 {
    panic::catch_unwind(AssertUnwindSafe(work)).unwrap_or(INVALID_DATA)
}

/// # Safety
///
/// `data` must point to `len` readable bytes, or `len` must be 0.
unsafe fn input<'a>(data: *const u8, len: usize) -> &'a [u8] {
    if len == 0 {
        return &[];
    }
    // SAFETY: the caller passes a pinned span of `len` bytes.
    unsafe { slice::from_raw_parts(data, len) }
}

/// # Safety
///
/// `data` must point to `len` writable bytes, or `len` must be 0.
unsafe fn output<'a>(data: *mut u8, len: usize) -> &'a mut [u8] {
    if len == 0 {
        return &mut [];
    }
    // SAFETY: the caller passes a pinned span of `len` bytes.
    unsafe { slice::from_raw_parts_mut(data, len) }
}

fn options(quality: i32, window: i32) -> Option<Options> {
    if !(0..=11).contains(&quality) || !(10..=24).contains(&window) {
        return None;
    }
    Options::default()
        .with_quality(quality.min(MAX_QUALITY) as u8)
        .and_then(|options| options.with_window_bits(window as u8))
        .ok()
}

/// Streaming encoder behind .NET `BrotliEncoder`.
pub struct Encoder {
    /// `None` after the finish operation.
    stream: Option<StreamEncoder<Vec<u8>>>,
    /// Output not yet copied to a destination.
    pending: Vec<u8>,
    pending_pos: usize,
}

impl Encoder {
    /// Copies pending output to `destination`; returns the bytes copied.
    fn drain(&mut self, destination: &mut [u8]) -> usize {
        let pending = &self.pending[self.pending_pos..];
        let count = pending.len().min(destination.len());
        destination[..count].copy_from_slice(&pending[..count]);
        self.pending_pos += count;
        count
    }

    fn compress(
        &mut self,
        source: &[u8],
        destination: &mut [u8],
        operation: i32,
    ) -> (i32, usize, usize) {
        let mut written = self.drain(destination);
        if self.pending_pos < self.pending.len() {
            return (DESTINATION_TOO_SMALL, 0, written);
        }
        let Some(encoder) = self.stream.as_mut() else {
            let status = if source.is_empty() {
                DONE
            } else {
                INVALID_DATA
            };
            return (status, 0, written);
        };
        if encoder.write_all(source).is_err() {
            return (INVALID_DATA, 0, written);
        }
        let output = if operation == OPERATION_FINISH {
            match self.stream.take().map(StreamEncoder::finish) {
                Some(Ok(output)) => output,
                _ => return (INVALID_DATA, source.len(), written),
            }
        } else {
            if operation == OPERATION_FLUSH && encoder.flush().is_err() {
                return (INVALID_DATA, source.len(), written);
            }
            std::mem::take(encoder.get_mut())
        };
        self.pending = output;
        self.pending_pos = 0;
        written += self.drain(&mut destination[written..]);
        let status = if self.pending_pos < self.pending.len() {
            DESTINATION_TOO_SMALL
        } else {
            DONE
        };
        (status, source.len(), written)
    }
}

/// Returns null for a quality outside 0 to 11 or a window outside 10 to 24.
#[unsafe(no_mangle)]
pub extern "C" fn burli_encoder_create(quality: i32, window: i32) -> *mut Encoder {
    panic::catch_unwind(|| {
        let Some(options) = options(quality, window) else {
            return std::ptr::null_mut();
        };
        match StreamEncoder::with_options(Vec::new(), options) {
            Ok(encoder) => Box::into_raw(Box::new(Encoder {
                stream: Some(encoder),
                pending: Vec::new(),
                pending_pos: 0,
            })),
            Err(_) => std::ptr::null_mut(),
        }
    })
    .unwrap_or(std::ptr::null_mut())
}

/// # Safety
///
/// `encoder` must come from [`burli_encoder_create`] and not be used again.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn burli_encoder_destroy(encoder: *mut Encoder) {
    if !encoder.is_null() {
        // SAFETY: the handle owns the box, and this is its last use.
        let _ = panic::catch_unwind(AssertUnwindSafe(|| drop(unsafe { Box::from_raw(encoder) })));
    }
}

/// Compresses `source`, then runs `operation`. Takes all of `source` unless
/// earlier output still waits for room.
///
/// # Safety
///
/// `encoder` must be a live handle used by no other thread meanwhile. The
/// pointer and length pairs must describe valid memory, and `consumed` and
/// `written` must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn burli_encoder_compress(
    encoder: *mut Encoder,
    source: *const u8,
    source_len: usize,
    destination: *mut u8,
    destination_len: usize,
    consumed: *mut usize,
    written: *mut usize,
    operation: i32,
) -> i32 {
    guard(|| {
        // SAFETY: the caller's contract above.
        let (encoder, source, destination) = unsafe {
            (
                &mut *encoder,
                input(source, source_len),
                output(destination, destination_len),
            )
        };
        let (status, taken, count) = encoder.compress(source, destination, operation);
        // SAFETY: the caller's contract above.
        unsafe {
            consumed.write(taken);
            written.write(count);
        }
        status
    })
}

/// Streaming decoder behind .NET `BrotliDecoder`.
pub struct Decoder(PushDecoder);

#[unsafe(no_mangle)]
pub extern "C" fn burli_decoder_create() -> *mut Decoder {
    Box::into_raw(Box::new(Decoder(PushDecoder::new())))
}

/// # Safety
///
/// `decoder` must come from [`burli_decoder_create`] and not be used again.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn burli_decoder_destroy(decoder: *mut Decoder) {
    if !decoder.is_null() {
        // SAFETY: the handle owns the box, and this is its last use.
        let _ = panic::catch_unwind(AssertUnwindSafe(|| drop(unsafe { Box::from_raw(decoder) })));
    }
}

/// # Safety
///
/// As for [`burli_encoder_compress`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn burli_decoder_decompress(
    decoder: *mut Decoder,
    source: *const u8,
    source_len: usize,
    destination: *mut u8,
    destination_len: usize,
    consumed: *mut usize,
    written: *mut usize,
) -> i32 {
    guard(|| {
        // SAFETY: the caller's contract above.
        let (decoder, source, destination) = unsafe {
            (
                &mut *decoder,
                input(source, source_len),
                output(destination, destination_len),
            )
        };
        let (status, taken, count) = match decoder.0.decompress(source, destination) {
            Ok(progress) => (
                match progress.status {
                    DecodeStatus::Finished => DONE,
                    DecodeStatus::NeedsOutput => DESTINATION_TOO_SMALL,
                    DecodeStatus::NeedsInput => NEED_MORE_DATA,
                },
                progress.consumed,
                progress.written,
            ),
            Err(_) => (INVALID_DATA, 0, 0),
        };
        // SAFETY: the caller's contract above.
        unsafe {
            consumed.write(taken);
            written.write(count);
        }
        status
    })
}

/// Compresses all of `source` into `destination`.
///
/// # Safety
///
/// The pointer and length pairs must describe valid memory, and `written`
/// must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn burli_compress(
    source: *const u8,
    source_len: usize,
    destination: *mut u8,
    destination_len: usize,
    written: *mut usize,
    quality: i32,
    window: i32,
) -> i32 {
    guard(|| {
        let Some(options) = options(quality, window) else {
            return INVALID_DATA;
        };
        // SAFETY: the caller's contract above.
        let (source, destination) = unsafe {
            (
                input(source, source_len),
                output(destination, destination_len),
            )
        };
        let Ok(encoded) = burli::compress_with_options(source, &options) else {
            return INVALID_DATA;
        };
        if encoded.len() > destination.len() {
            return DESTINATION_TOO_SMALL;
        }
        destination[..encoded.len()].copy_from_slice(&encoded);
        // SAFETY: the caller's contract above.
        unsafe { written.write(encoded.len()) };
        DONE
    })
}

/// Decompresses all of `source`, a whole stream, into `destination`.
///
/// # Safety
///
/// As for [`burli_compress`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn burli_decompress(
    source: *const u8,
    source_len: usize,
    destination: *mut u8,
    destination_len: usize,
    written: *mut usize,
) -> i32 {
    guard(|| {
        // SAFETY: the caller's contract above.
        let (source, destination) = unsafe {
            (
                input(source, source_len),
                output(destination, destination_len),
            )
        };
        match burli::decompress_into_slice(source, destination) {
            Ok(count) => {
                // SAFETY: the caller's contract above.
                unsafe { written.write(count) };
                DONE
            }
            Err(BurliError::OutputLimitExceeded { .. }) => DESTINATION_TOO_SMALL,
            Err(_) => INVALID_DATA,
        }
    })
}
