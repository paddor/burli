//! Node.js binding for burli. `index.js` shapes it like the Brotli
//! functions of `node:zlib`.
//!
//! Async work runs on the libuv thread pool. Error messages start with the
//! code `index.js` gives the error.

use std::io::Write;
use std::sync::{Arc, Mutex, MutexGuard};

use burli::{BurliError, DecodeStatus, Mode, Options, PushDecoder, StreamEncoder};
use napi::bindgen_prelude::{AsyncTask, Buffer};
use napi::{Env, Error, Result, Task};
use napi_derive::napi;

/// The highest quality the encoder implements. Higher ones run as this.
const MAX_QUALITY: u32 = 5;
/// The first output buffer of a decompress step. Later ones double it.
const FIRST_OUTPUT_ROOM: usize = 32 * 1024;

const TRUNCATED: &str = "Z_BUF_ERROR: unexpected end of file";

fn error(code: &str, message: impl std::fmt::Display) -> Error {
    Error::from_reason(format!("{code}: {message}"))
}

fn decode_error(err: &BurliError) -> Error {
    match err {
        BurliError::OutputLimitExceeded { .. } => error("ERR_BUFFER_TOO_LARGE", err),
        _ => error("ERR_BURLI_DECOMPRESSION_FAILED", err),
    }
}

/// Builds encoder options from `BROTLI_PARAM_*` values that `index.js`
/// clamped into Brotli's ranges.
fn encode_options(mode: u32, quality: u32, lgwin: u32, lgblock: u32) -> Result<Options> {
    let mode = match mode {
        1 => Mode::Text,
        2 => Mode::Font,
        _ => Mode::Generic,
    };
    let options = Options::default()
        .with_quality(quality.min(MAX_QUALITY) as u8)
        .and_then(|options| options.with_window_bits(lgwin as u8))
        .and_then(|options| options.with_block_bits((lgblock != 0).then_some(lgblock as u8)))
        .map_err(|err| error("ERR_BROTLI_INVALID_PARAM", err))?;
    Ok(options.with_mode(mode))
}

/// Decodes a whole stream. Data after its end is an error.
fn decompress_all(input: &[u8], max_output_length: f64) -> Result<Vec<u8>> {
    // Saturates; JavaScript limits stay below `usize::MAX`.
    burli::decompress_with_limit(input, max_output_length as usize).map_err(|err| match err {
        BurliError::Format("unexpected end of Brotli input") => Error::from_reason(TRUNCATED),
        _ => decode_error(&err),
    })
}

#[napi(js_name = "compressSync")]
pub fn compress_sync(
    input: &[u8],
    mode: u32,
    quality: u32,
    lgwin: u32,
    lgblock: u32,
) -> Result<Buffer> {
    let options = encode_options(mode, quality, lgwin, lgblock)?;
    let encoded = burli::compress_with_options(input, &options)
        .map_err(|err| error("ERR_BURLI_COMPRESSION_FAILED", err))?;
    Ok(encoded.into())
}

#[napi(js_name = "decompressSync")]
pub fn decompress_sync(input: &[u8], max_output_length: f64) -> Result<Buffer> {
    Ok(decompress_all(input, max_output_length)?.into())
}

pub struct CompressTask {
    input: Vec<u8>,
    options: Options,
}

impl Task for CompressTask {
    type Output = Vec<u8>;
    type JsValue = Buffer;

    fn compute(&mut self) -> Result<Vec<u8>> {
        burli::compress_with_options(&self.input, &self.options)
            .map_err(|err| error("ERR_BURLI_COMPRESSION_FAILED", err))
    }

    fn resolve(&mut self, _env: Env, output: Vec<u8>) -> Result<Buffer> {
        Ok(output.into())
    }
}

/// Compresses a copy of `input`, which JavaScript may change meanwhile.
#[napi]
pub fn compress(
    input: &[u8],
    mode: u32,
    quality: u32,
    lgwin: u32,
    lgblock: u32,
) -> Result<AsyncTask<CompressTask>> {
    Ok(AsyncTask::new(CompressTask {
        input: input.to_vec(),
        options: encode_options(mode, quality, lgwin, lgblock)?,
    }))
}

pub struct DecompressTask {
    input: Vec<u8>,
    max_output_length: f64,
}

impl Task for DecompressTask {
    type Output = Vec<u8>;
    type JsValue = Buffer;

    fn compute(&mut self) -> Result<Vec<u8>> {
        decompress_all(&self.input, self.max_output_length)
    }

    fn resolve(&mut self, _env: Env, output: Vec<u8>) -> Result<Buffer> {
        Ok(output.into())
    }
}

/// Decompresses a copy of `input`, which JavaScript may change meanwhile.
#[napi]
pub fn decompress(input: &[u8], max_output_length: f64) -> AsyncTask<DecompressTask> {
    AsyncTask::new(DecompressTask {
        input: input.to_vec(),
        max_output_length,
    })
}

fn lock<T>(state: &Mutex<T>) -> Result<MutexGuard<'_, T>> {
    state
        .lock()
        .map_err(|_| error("ERR_BURLI_FAILED", "an earlier call panicked"))
}

/// `BROTLI_OPERATION_*` values.
const OPERATION_FLUSH: u32 = 1;
const OPERATION_FINISH: u32 = 2;

/// Streaming compression for `BrotliCompress`. Calls must not overlap.
#[napi]
pub struct Compressor {
    /// `None` after the finish operation.
    encoder: Arc<Mutex<Option<StreamEncoder<Vec<u8>>>>>,
}

#[napi]
impl Compressor {
    #[napi(constructor)]
    pub fn new(mode: u32, quality: u32, lgwin: u32, lgblock: u32) -> Result<Self> {
        let options = encode_options(mode, quality, lgwin, lgblock)?;
        let encoder = StreamEncoder::with_options(Vec::new(), options)
            .map_err(|err| error("ERR_BROTLI_INVALID_PARAM", err))?;
        Ok(Self {
            encoder: Arc::new(Mutex::new(Some(encoder))),
        })
    }

    /// Compresses a copy of `input`, then runs `operation`, a
    /// `BROTLI_OPERATION_*` value. Resolves to the output produced so far.
    #[napi]
    pub fn write(&self, input: &[u8], operation: u32) -> AsyncTask<CompressStep> {
        AsyncTask::new(CompressStep {
            encoder: Arc::clone(&self.encoder),
            input: input.to_vec(),
            operation,
        })
    }
}

pub struct CompressStep {
    encoder: Arc<Mutex<Option<StreamEncoder<Vec<u8>>>>>,
    input: Vec<u8>,
    operation: u32,
}

impl Task for CompressStep {
    type Output = Vec<u8>;
    type JsValue = Buffer;

    fn compute(&mut self) -> Result<Vec<u8>> {
        let mut state = lock(&self.encoder)?;
        let failed = |err: &dyn std::fmt::Display| error("ERR_BURLI_COMPRESSION_FAILED", err);
        let encoder = state.as_mut().ok_or_else(|| failed(&"write after end"))?;
        encoder.write_all(&self.input).map_err(|err| failed(&err))?;
        match self.operation {
            OPERATION_FINISH => {
                let encoder = state.take().expect("checked above");
                return encoder.finish().map_err(|err| failed(&err));
            }
            OPERATION_FLUSH => encoder.flush().map_err(|err| failed(&err))?,
            _ => {}
        }
        Ok(std::mem::take(encoder.get_mut()))
    }

    fn resolve(&mut self, _env: Env, output: Vec<u8>) -> Result<Buffer> {
        Ok(output.into())
    }
}

struct DecoderState {
    decoder: PushDecoder,
    /// Input the last call could not hand to the decoder before it reached
    /// its output limit.
    tail: Vec<u8>,
}

/// Streaming decompression for `BrotliDecompress`. Calls must not overlap.
#[napi]
pub struct Decompressor {
    state: Arc<Mutex<DecoderState>>,
}

#[napi(object)]
pub struct DecompressStepResult {
    pub output: Buffer,
    /// The decoder holds more output; call again with empty input.
    pub more: bool,
    pub finished: bool,
}

#[napi]
impl Decompressor {
    #[napi(constructor)]
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(DecoderState {
                decoder: PushDecoder::new(),
                tail: Vec::new(),
            })),
        }
    }

    /// Decompresses a copy of `input` into at most `limit` bytes. While the
    /// result says `more`, pass empty input.
    #[napi]
    pub fn write(&self, input: &[u8], limit: u32) -> AsyncTask<DecompressStep> {
        AsyncTask::new(DecompressStep {
            state: Arc::clone(&self.state),
            input: input.to_vec(),
            limit: limit.max(1) as usize,
        })
    }

    /// Whether the stream is decoded and all of its output returned.
    #[napi(getter)]
    pub fn finished(&self) -> Result<bool> {
        Ok(lock(&self.state)?.decoder.is_finished())
    }
}

pub struct DecompressStep {
    state: Arc<Mutex<DecoderState>>,
    input: Vec<u8>,
    limit: usize,
}

impl Task for DecompressStep {
    type Output = (Vec<u8>, bool, bool);
    type JsValue = DecompressStepResult;

    fn compute(&mut self) -> Result<Self::Output> {
        let mut state = lock(&self.state)?;
        let state = &mut *state;
        if state.decoder.is_finished() && !self.input.is_empty() {
            return Err(error(
                "ERR_BURLI_DECOMPRESSION_FAILED",
                "data after the end of the Brotli stream",
            ));
        }
        if !state.tail.is_empty() {
            if !self.input.is_empty() {
                return Err(error("ERR_BURLI_FAILED", "input while output is pending"));
            }
            self.input = std::mem::take(&mut state.tail);
        }
        let mut output = Vec::new();
        let mut consumed = 0;
        loop {
            let start = output.len();
            let room = (self.limit - start).min(start.max(FIRST_OUTPUT_ROOM));
            output.resize(start + room, 0);
            let progress = state
                .decoder
                .decompress(&self.input[consumed..], &mut output[start..])
                .map_err(|err| decode_error(&err))?;
            output.truncate(start + progress.written);
            consumed += progress.consumed;
            match progress.status {
                DecodeStatus::NeedsOutput if output.len() < self.limit => {}
                DecodeStatus::NeedsOutput => {
                    state.tail = self.input[consumed..].to_vec();
                    return Ok((output, true, false));
                }
                DecodeStatus::NeedsInput => return Ok((output, false, false)),
                DecodeStatus::Finished if consumed < self.input.len() => {
                    return Err(error(
                        "ERR_BURLI_DECOMPRESSION_FAILED",
                        "data after the end of the Brotli stream",
                    ));
                }
                DecodeStatus::Finished => return Ok((output, false, true)),
            }
        }
    }

    fn resolve(
        &mut self,
        _env: Env,
        (output, more, finished): Self::Output,
    ) -> Result<Self::JsValue> {
        Ok(DecompressStepResult {
            output: output.into(),
            more,
            finished,
        })
    }
}
