//! Python binding for burli, shaped like Google's `brotli` module.
//!
//! Qualities 6 to 11 run as 5. Work on the input runs without the GIL.

use std::io::Write;
use std::ops::Deref;

use burli::{BurliError, DecodeStatus, Mode, Options, PushDecoder, StreamEncoder};
use pyo3::exceptions::PyTypeError;
use pyo3::prelude::*;
use pyo3::pybacked::PyBackedBytes;
use pyo3::types::{PyBytes, PyMemoryView};

use exception::error;

mod exception {
    // The name matches `brotli.error`.
    #![allow(non_camel_case_types)]

    pyo3::create_exception!(
        burli,
        error,
        pyo3::exceptions::PyException,
        "An error occurred in the Brotli implementation."
    );
}

const MODE_GENERIC: i64 = 0;
const MODE_TEXT: i64 = 1;
const MODE_FONT: i64 = 2;
/// The highest quality the encoder implements. Higher ones run as this.
const MAX_QUALITY: i64 = 5;
/// The first output buffer of a decompress call. Later ones double it.
const FIRST_OUTPUT_ROOM: usize = 32 * 1024;

fn burli_error(err: &BurliError) -> PyErr {
    error::new_err(format!("burli: {err}"))
}

/// The bytes of a buffer argument: borrowed from `bytes`, copied from a
/// `bytearray` or any other C-contiguous buffer, which other threads may
/// change while the GIL is released.
enum Data {
    Backed(PyBackedBytes),
    Copied(Vec<u8>),
}

impl Deref for Data {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        match self {
            Self::Backed(bytes) => bytes,
            Self::Copied(bytes) => bytes,
        }
    }
}

fn data(object: &Bound<'_, PyAny>) -> PyResult<Data> {
    if let Ok(bytes) = object.extract::<PyBackedBytes>() {
        return Ok(Data::Backed(bytes));
    }
    // The buffer protocol is not in the stable ABI before Python 3.11.
    let invalid = || PyTypeError::new_err("burli: data must be a C-contiguous buffer");
    let view = PyMemoryView::from(object).map_err(|_| invalid())?;
    if !view.getattr("c_contiguous")?.is_truthy()? {
        return Err(invalid());
    }
    let bytes = view.call_method0("tobytes")?.extract::<PyBackedBytes>()?;
    Ok(Data::Copied(bytes.to_vec()))
}

fn encode_options(mode: i64, quality: i64, lgwin: i64, lgblock: i64) -> PyResult<Options> {
    let mode = match mode {
        MODE_GENERIC => Mode::Generic,
        MODE_TEXT => Mode::Text,
        MODE_FONT => Mode::Font,
        _ => return Err(error::new_err("burli: invalid mode")),
    };
    if !(0..=11).contains(&quality) {
        return Err(error::new_err("burli: invalid quality; range is 0 to 11"));
    }
    if !(10..=24).contains(&lgwin) {
        return Err(error::new_err("burli: invalid lgwin; range is 10 to 24"));
    }
    if lgblock != 0 && !(16..=24).contains(&lgblock) {
        return Err(error::new_err(
            "burli: invalid lgblock; range is 16 to 24, or 0",
        ));
    }
    let options = Options::default()
        .with_quality(quality.min(MAX_QUALITY) as u8)
        .and_then(|options| options.with_window_bits(lgwin as u8))
        .and_then(|options| options.with_block_bits((lgblock != 0).then_some(lgblock as u8)))
        .map_err(|err| burli_error(&err))?;
    Ok(options.with_mode(mode))
}

/// Compress a byte string.
///
/// Qualities 6 to 11 run as 5.
#[pyfunction]
#[pyo3(signature = (string, mode = MODE_GENERIC, quality = MAX_QUALITY, lgwin = 22, lgblock = 0))]
fn compress<'py>(
    py: Python<'py>,
    string: &Bound<'py, PyAny>,
    mode: i64,
    quality: i64,
    lgwin: i64,
    lgblock: i64,
) -> PyResult<Bound<'py, PyBytes>> {
    let options = encode_options(mode, quality, lgwin, lgblock)?;
    let input = data(string)?;
    let encoded = py
        .detach(|| burli::compress_with_options(&input, &options))
        .map_err(|err| burli_error(&err))?;
    Ok(PyBytes::new(py, &encoded))
}

/// Decompress a compressed byte string.
#[pyfunction]
fn decompress<'py>(py: Python<'py>, string: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyBytes>> {
    let input = data(string)?;
    let decoded = py
        .detach(|| burli::decompress(&input))
        .map_err(|err| burli_error(&err))?;
    Ok(PyBytes::new(py, &decoded))
}

/// An object to compress a byte string.
#[pyclass(module = "burli")]
struct Compressor {
    /// `None` after `finish`.
    encoder: Option<StreamEncoder<Vec<u8>>>,
}

impl Compressor {
    fn encoder(&mut self) -> PyResult<&mut StreamEncoder<Vec<u8>>> {
        self.encoder
            .as_mut()
            .ok_or_else(|| error::new_err("burli: compressor is finished"))
    }
}

/// Returns the bytes the encoder wrote so far and clears them.
fn take_output<'py>(py: Python<'py>, encoder: &mut StreamEncoder<Vec<u8>>) -> Bound<'py, PyBytes> {
    let output = PyBytes::new(py, encoder.get_ref());
    encoder.get_mut().clear();
    output
}

#[pymethods]
impl Compressor {
    #[new]
    #[pyo3(signature = (mode = MODE_GENERIC, quality = MAX_QUALITY, lgwin = 22, lgblock = 0))]
    fn new(mode: i64, quality: i64, lgwin: i64, lgblock: i64) -> PyResult<Self> {
        let options = encode_options(mode, quality, lgwin, lgblock)?;
        let encoder =
            StreamEncoder::with_options(Vec::new(), options).map_err(|err| burli_error(&err))?;
        Ok(Self {
            encoder: Some(encoder),
        })
    }

    /// Compress `data`, returning the compressed output produced so far.
    fn process<'py>(
        &mut self,
        py: Python<'py>,
        data: &Bound<'py, PyAny>,
    ) -> PyResult<Bound<'py, PyBytes>> {
        let input = self::data(data)?;
        let encoder = self.encoder()?;
        py.detach(|| encoder.write_all(&input))
            .map_err(|err| error::new_err(format!("burli: {err}")))?;
        Ok(take_output(py, encoder))
    }

    /// Compress all pending input, returning output that decodes to all
    /// input so far.
    fn flush<'py>(&mut self, py: Python<'py>) -> PyResult<Bound<'py, PyBytes>> {
        let encoder = self.encoder()?;
        py.detach(|| encoder.flush())
            .map_err(|err| error::new_err(format!("burli: {err}")))?;
        Ok(take_output(py, encoder))
    }

    /// Compress all pending input and end the stream. The compressor takes
    /// no more input afterwards.
    fn finish<'py>(&mut self, py: Python<'py>) -> PyResult<Bound<'py, PyBytes>> {
        let encoder = self
            .encoder
            .take()
            .ok_or_else(|| error::new_err("burli: compressor is finished"))?;
        let output = py
            .detach(|| encoder.finish())
            .map_err(|err| burli_error(&err))?;
        Ok(PyBytes::new(py, &output))
    }
}

/// An object to decompress a byte string.
#[pyclass(module = "burli")]
struct Decompressor {
    decoder: PushDecoder,
    /// Input the last call could not hand to the decoder before it reached
    /// its output limit.
    tail: Vec<u8>,
    /// The decoder holds output the last call did not return.
    pending: bool,
    /// A call failed; the decoder takes no more calls.
    failed: bool,
}

impl Decompressor {
    fn check(&self) -> PyResult<()> {
        if self.failed {
            return Err(error::new_err("burli: decoder is unhealthy"));
        }
        Ok(())
    }
}

/// Decodes `input` into a buffer that grows up to `limit` bytes. Returns
/// the output, the input bytes taken, and the decoder status.
fn decode(
    decoder: &mut PushDecoder,
    input: &[u8],
    limit: usize,
) -> Result<(Vec<u8>, usize, DecodeStatus), BurliError> {
    let mut output = Vec::new();
    let mut consumed = 0;
    loop {
        let start = output.len();
        let room = (limit - start).min(start.max(FIRST_OUTPUT_ROOM));
        output.resize(start + room, 0);
        let progress = decoder.decompress(&input[consumed..], &mut output[start..])?;
        output.truncate(start + progress.written);
        consumed += progress.consumed;
        if progress.status != DecodeStatus::NeedsOutput || output.len() == limit {
            return Ok((output, consumed, progress.status));
        }
    }
}

#[pymethods]
impl Decompressor {
    #[new]
    fn new() -> Self {
        Self {
            decoder: PushDecoder::new(),
            tail: Vec::new(),
            pending: false,
            failed: false,
        }
    }

    /// Decompress `data`, returning the decompressed output produced so
    /// far. With `output_buffer_limit`, return at most that many bytes;
    /// then call with empty input until `can_accept_more_data()` returns
    /// `True` before passing more input.
    #[pyo3(signature = (data, output_buffer_limit = None))]
    fn process<'py>(
        &mut self,
        py: Python<'py>,
        data: &Bound<'py, PyAny>,
        output_buffer_limit: Option<isize>,
    ) -> PyResult<Bound<'py, PyBytes>> {
        self.check()?;
        let data = self::data(data)?;
        if !self.tail.is_empty() && !data.is_empty() {
            self.failed = true;
            return Err(error::new_err(
                "burli: decoder process called with data when 'can_accept_more_data()' is False",
            ));
        }
        let tail = std::mem::take(&mut self.tail);
        let input: &[u8] = if tail.is_empty() { &data } else { &tail };
        // Each call returns some output, so draining calls make progress.
        let limit = output_buffer_limit.map_or(usize::MAX, |limit| limit.max(1) as usize);
        let decoder = &mut self.decoder;
        let result = py.detach(|| decode(decoder, input, limit));
        let (output, consumed, status) = result.map_err(|err| {
            self.failed = true;
            burli_error(&err)
        })?;
        match status {
            DecodeStatus::Finished if consumed < input.len() => {
                self.failed = true;
                return Err(error::new_err(
                    "burli: data after the end of the Brotli stream",
                ));
            }
            DecodeStatus::NeedsOutput => self.tail = input[consumed..].to_vec(),
            _ => {}
        }
        self.pending = status == DecodeStatus::NeedsOutput;
        Ok(PyBytes::new(py, &output))
    }

    /// Whether the decoder reached the end of the stream and returned all
    /// of its output.
    fn is_finished(&self) -> PyResult<bool> {
        self.check()?;
        Ok(self.decoder.is_finished())
    }

    /// Whether the decoder takes more input. `False` while it holds input
    /// or output from a call that reached `output_buffer_limit`.
    fn can_accept_more_data(&self) -> PyResult<bool> {
        self.check()?;
        Ok(self.tail.is_empty() && !self.pending)
    }
}

#[pymodule]
fn _burli(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add("error", module.py().get_type::<error>())?;
    module.add("MODE_GENERIC", MODE_GENERIC)?;
    module.add("MODE_TEXT", MODE_TEXT)?;
    module.add("MODE_FONT", MODE_FONT)?;
    module.add("__version__", env!("CARGO_PKG_VERSION"))?;
    module.add_function(wrap_pyfunction!(compress, module)?)?;
    module.add_function(wrap_pyfunction!(decompress, module)?)?;
    module.add_class::<Compressor>()?;
    module.add_class::<Decompressor>()?;
    Ok(())
}
