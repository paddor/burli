//! Ruby binding for burli, shaped like the `brotli` gem. `lib/burli.rb`
//! checks options and calls the `_native_*` methods here.
//!
//! Qualities 6 to 11 run as 5.

mod rb;

use std::ffi::c_void;
use std::io::Write;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Mutex, OnceLock, TryLockError};

use burli::decode::RawDictionary;
use burli::{BurliError, DecodeStatus, Mode, Options, PushDecoder, StreamEncoder};
use rb_sys::{VALUE, rb_data_type_struct__bindgen_ty_1, rb_data_type_t, size_t};

use crate::rb::{RbResult, RubyErr};

/// The highest quality the encoder implements. Higher ones run as this.
const MAX_QUALITY: i64 = 5;
/// The first output buffer of a decompress call. Later ones double it.
const FIRST_OUTPUT_ROOM: usize = 32 * 1024;
/// Work on fewer input and output bytes keeps the GVL, as in zrip. A
/// decompression without an output limit always releases it: its output
/// does not follow from its input.
const GVL_THRESHOLD: usize = 64 * 1024;

#[derive(Clone, Copy)]
struct GlobalValue(VALUE);

// SAFETY: the value is a class constant, which Ruby never frees or moves.
unsafe impl Send for GlobalValue {}
// SAFETY: as above.
unsafe impl Sync for GlobalValue {}

static ERROR: OnceLock<GlobalValue> = OnceLock::new();

/// A `Burli::Error`.
fn error(message: impl Into<String>) -> RubyErr {
    RubyErr::new(ERROR.get().expect("Burli::Error is defined").0, message)
}

fn burli_error(err: &BurliError) -> RubyErr {
    error(err.to_string())
}

fn encode_options(mode: VALUE, quality: VALUE, lgwin: VALUE, lgblock: VALUE) -> RbResult<Options> {
    let mode = match rb::value_to_i64(mode)? {
        1 => Mode::Text,
        2 => Mode::Font,
        _ => Mode::Generic,
    };
    let quality = rb::value_to_i64(quality)?.clamp(0, MAX_QUALITY) as u8;
    let lgwin =
        u8::try_from(rb::value_to_i64(lgwin)?).map_err(|_| RubyErr::arg("invalid lgwin"))?;
    let lgblock =
        u8::try_from(rb::value_to_i64(lgblock)?).map_err(|_| RubyErr::arg("invalid lgblock"))?;
    let options = Options::default()
        .with_quality(quality)
        .and_then(|options| options.with_window_bits(lgwin))
        .and_then(|options| options.with_block_bits((lgblock != 0).then_some(lgblock)))
        .map_err(|err| RubyErr::arg(err.to_string()))?;
    Ok(options.with_mode(mode))
}

/// Runs `func` on the value behind `mutex`, without the GVL when
/// `release_gvl`. Waits for a held lock without the GVL too, so the thread
/// holding it can finish.
fn with_mutex<T, R, F>(
    mutex: &Mutex<T>,
    release_gvl: bool,
    input: &mut rb::InputBytes,
    func: F,
) -> RbResult<R>
where
    F: FnOnce(&mut T, &[u8]) -> RbResult<R>,
{
    let poisoned = || RubyErr::runtime("an earlier call panicked");
    if !release_gvl {
        match mutex.try_lock() {
            Ok(mut guard) => return func(&mut guard, input.as_slice()),
            Err(TryLockError::Poisoned(_)) => return Err(poisoned()),
            Err(TryLockError::WouldBlock) => {}
        }
    }
    input.lock();
    rb::maybe_without_gvl(true, || {
        let mut guard = mutex.lock().map_err(|_| poisoned())?;
        func(&mut guard, input.as_slice())
    })?
}

// ---------- module functions ----------

fn deflate_impl(
    input: VALUE,
    mode: VALUE,
    quality: VALUE,
    lgwin: VALUE,
    lgblock: VALUE,
) -> RbResult<VALUE> {
    let options = encode_options(mode, quality, lgwin, lgblock)?;
    let mut input = rb::input_bytes(input)?;
    let release_gvl = input.len() >= GVL_THRESHOLD;
    if release_gvl {
        input.lock();
    }
    let encoded = rb::maybe_without_gvl(release_gvl, || {
        burli::compress_with_options(input.as_slice(), &options)
    })?
    .map_err(|err| burli_error(&err))?;
    rb::new_binary_string(&encoded)
}

fn inflate_impl(input: VALUE, dictionary: VALUE) -> RbResult<VALUE> {
    let dictionary = rb::value_to_option_bytes(dictionary)?.map(RawDictionary::from_vec);
    let mut input = rb::input_bytes(input)?;
    input.lock();
    let decoded = rb::maybe_without_gvl(true, || match &dictionary {
        Some(dictionary) => burli::decompress_with_raw_dictionary(input.as_slice(), dictionary),
        None => burli::decompress(input.as_slice()),
    })?
    .map_err(|err| burli_error(&err))?;
    rb::new_binary_string(&decoded)
}

unsafe extern "C" fn deflate(
    _module: VALUE,
    input: VALUE,
    mode: VALUE,
    quality: VALUE,
    lgwin: VALUE,
    lgblock: VALUE,
) -> VALUE {
    rb::wrap(|| deflate_impl(input, mode, quality, lgwin, lgblock))
}

unsafe extern "C" fn inflate(_module: VALUE, input: VALUE, dictionary: VALUE) -> VALUE {
    rb::wrap(|| inflate_impl(input, dictionary))
}

// ---------- typed data ----------

struct NativeDataType(rb_data_type_t);

// SAFETY: the type is built once and never changed.
unsafe impl Send for NativeDataType {}
// SAFETY: as above.
unsafe impl Sync for NativeDataType {}

static COMPRESSOR_DATA_TYPE: OnceLock<NativeDataType> = OnceLock::new();
static DECOMPRESSOR_DATA_TYPE: OnceLock<NativeDataType> = OnceLock::new();

fn data_type(
    cell: &'static OnceLock<NativeDataType>,
    name: &'static std::ffi::CStr,
    free: unsafe extern "C" fn(*mut c_void),
    size: unsafe extern "C" fn(*const c_void) -> size_t,
) -> *const rb_data_type_t {
    &raw const cell
        .get_or_init(|| {
            NativeDataType(rb_data_type_t {
                wrap_struct_name: name.as_ptr(),
                function: rb_data_type_struct__bindgen_ty_1 {
                    dmark: None,
                    dfree: Some(free),
                    dsize: Some(size),
                    dcompact: None,
                    reserved: [std::ptr::null_mut(); 1],
                },
                parent: std::ptr::null(),
                data: std::ptr::null_mut(),
                // RUBY_TYPED_FREE_IMMEDIATELY: freeing runs no Ruby code.
                flags: 1,
            })
        })
        .0
}

fn compressor_data_type() -> *const rb_data_type_t {
    data_type(
        &COMPRESSOR_DATA_TYPE,
        c"burli_compressor",
        free::<Compressor>,
        native_size::<Compressor>,
    )
}

fn decompressor_data_type() -> *const rb_data_type_t {
    data_type(
        &DECOMPRESSOR_DATA_TYPE,
        c"burli_decompressor",
        free::<Decompressor>,
        native_size::<Decompressor>,
    )
}

unsafe extern "C" fn free<T>(data: *mut c_void) {
    if !data.is_null() {
        // SAFETY: the object owned this box, and Ruby frees it once.
        let _ = catch_unwind(AssertUnwindSafe(|| {
            drop(unsafe { Box::from_raw(data.cast::<T>()) });
        }));
    }
}

unsafe extern "C" fn native_size<T>(_data: *const c_void) -> size_t {
    std::mem::size_of::<T>() as size_t
}

// ---------- Compressor ----------

struct Compressor {
    /// `None` after `finish`.
    encoder: Mutex<Option<StreamEncoder<Vec<u8>>>>,
}

fn compressor_ref<'a>(value: VALUE) -> RbResult<&'a Compressor> {
    // SAFETY: the data type wraps a `Compressor`, and Ruby keeps `value`
    // alive while the method runs.
    unsafe { rb::typed_data_ref(value, compressor_data_type()) }
}

fn compressor_new_impl(
    class: VALUE,
    mode: VALUE,
    quality: VALUE,
    lgwin: VALUE,
    lgblock: VALUE,
) -> RbResult<VALUE> {
    let options = encode_options(mode, quality, lgwin, lgblock)?;
    let encoder =
        StreamEncoder::with_options(Vec::new(), options).map_err(|err| burli_error(&err))?;
    let compressor = Box::new(Compressor {
        encoder: Mutex::new(Some(encoder)),
    });
    // SAFETY: the data type frees a `Box<Compressor>`.
    unsafe { rb::wrap_typed_data(class, compressor, compressor_data_type()) }
}

/// Writes `input` to the encoder and runs `step`; returns the output so far.
fn compressor_run(
    rb_self: VALUE,
    input: VALUE,
    release_gvl: bool,
    step: fn(&mut StreamEncoder<Vec<u8>>) -> std::io::Result<()>,
) -> RbResult<VALUE> {
    let compressor = compressor_ref(rb_self)?;
    let mut input = if input == rb::qnil() {
        rb::InputBytes::Owned(Vec::new())
    } else {
        rb::input_bytes(input)?
    };
    let release_gvl = release_gvl || input.len() >= GVL_THRESHOLD;
    let output = with_mutex(
        &compressor.encoder,
        release_gvl,
        &mut input,
        |encoder, input| {
            let encoder = encoder
                .as_mut()
                .ok_or_else(|| error("Compressor is finished"))?;
            encoder
                .write_all(input)
                .and_then(|()| step(encoder))
                .map_err(|err| error(err.to_string()))?;
            Ok(std::mem::take(encoder.get_mut()))
        },
    )?;
    rb::new_binary_string(&output)
}

fn compressor_finish_impl(rb_self: VALUE) -> RbResult<VALUE> {
    let compressor = compressor_ref(rb_self)?;
    let mut input = rb::InputBytes::Owned(Vec::new());
    let output = with_mutex(
        &compressor.encoder,
        true,
        &mut input,
        |encoder, _| match encoder.take() {
            Some(encoder) => encoder.finish().map_err(|err| burli_error(&err)),
            None => Ok(Vec::new()),
        },
    )?;
    rb::new_binary_string(&output)
}

fn compressor_is_finished_impl(rb_self: VALUE) -> RbResult<VALUE> {
    let compressor = compressor_ref(rb_self)?;
    let mut input = rb::InputBytes::Owned(Vec::new());
    let finished = with_mutex(&compressor.encoder, false, &mut input, |encoder, _| {
        Ok(encoder.is_none())
    })?;
    Ok(rb::bool_value(finished))
}

unsafe extern "C" fn compressor_new(
    class: VALUE,
    mode: VALUE,
    quality: VALUE,
    lgwin: VALUE,
    lgblock: VALUE,
) -> VALUE {
    rb::wrap(|| compressor_new_impl(class, mode, quality, lgwin, lgblock))
}

/// `process(input)`: compresses `input`, returning the output so far.
unsafe extern "C" fn compressor_process(rb_self: VALUE, input: VALUE) -> VALUE {
    rb::wrap(|| compressor_run(rb_self, input, false, |_| Ok(())))
}

/// `flush`: returns output that decodes to all input so far.
unsafe extern "C" fn compressor_flush(rb_self: VALUE) -> VALUE {
    rb::wrap(|| compressor_run(rb_self, rb::qnil(), true, Write::flush))
}

/// `finish`: ends the stream, returning the rest of the output.
unsafe extern "C" fn compressor_finish(rb_self: VALUE) -> VALUE {
    rb::wrap(|| compressor_finish_impl(rb_self))
}

unsafe extern "C" fn compressor_is_finished(rb_self: VALUE) -> VALUE {
    rb::wrap(|| compressor_is_finished_impl(rb_self))
}

// ---------- Decompressor ----------

struct Decompressor {
    state: Mutex<DecoderState>,
}

struct DecoderState {
    decoder: PushDecoder,
    /// Input the last call could not hand to the decoder before it reached
    /// its output limit.
    tail: Vec<u8>,
    /// The decoder holds output the last call did not return.
    pending: bool,
    /// Input after the end of the stream.
    unused: Vec<u8>,
    /// A call failed; the decoder takes no more calls.
    failed: bool,
}

impl DecoderState {
    fn check(&self) -> RbResult<()> {
        if self.failed {
            return Err(error("Decompressor failed on earlier input"));
        }
        Ok(())
    }

    /// Decodes `data`, or held input when `data` is empty, into at most
    /// `limit` bytes.
    fn process(&mut self, data: &[u8], limit: usize) -> RbResult<Vec<u8>> {
        self.check()?;
        if self.decoder.is_finished() {
            if data.is_empty() {
                return Ok(Vec::new());
            }
            return Err(error("Decompressor is finished"));
        }
        if (self.pending || !self.tail.is_empty()) && !data.is_empty() {
            return Err(error(
                "Decompressor cannot accept more data until pending output is drained",
            ));
        }
        let tail = std::mem::take(&mut self.tail);
        let input: &[u8] = if tail.is_empty() { data } else { &tail };
        let (output, consumed, status) =
            decode(&mut self.decoder, input, limit).map_err(|err| {
                self.failed = true;
                burli_error(&err)
            })?;
        match status {
            DecodeStatus::Finished => self.unused = input[consumed..].to_vec(),
            DecodeStatus::NeedsOutput => self.tail = input[consumed..].to_vec(),
            DecodeStatus::NeedsInput => {}
        }
        self.pending = status == DecodeStatus::NeedsOutput;
        Ok(output)
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

fn decompressor_ref<'a>(value: VALUE) -> RbResult<&'a Decompressor> {
    // SAFETY: the data type wraps a `Decompressor`, and Ruby keeps `value`
    // alive while the method runs.
    unsafe { rb::typed_data_ref(value, decompressor_data_type()) }
}

fn decompressor_new_impl(class: VALUE, dictionary: VALUE) -> RbResult<VALUE> {
    let decoder = match rb::value_to_option_bytes(dictionary)? {
        Some(dictionary) => PushDecoder::with_raw_dictionary(RawDictionary::from_vec(dictionary)),
        None => PushDecoder::new(),
    };
    let decompressor = Box::new(Decompressor {
        state: Mutex::new(DecoderState {
            decoder,
            tail: Vec::new(),
            pending: false,
            unused: Vec::new(),
            failed: false,
        }),
    });
    // SAFETY: the data type frees a `Box<Decompressor>`.
    unsafe { rb::wrap_typed_data(class, decompressor, decompressor_data_type()) }
}

fn decompressor_process_impl(rb_self: VALUE, input: VALUE, limit: VALUE) -> RbResult<VALUE> {
    let decompressor = decompressor_ref(rb_self)?;
    // Zero: no limit.
    let limit = match usize::try_from(rb::value_to_i64(limit)?) {
        Ok(0) => usize::MAX,
        Ok(limit) => limit,
        Err(_) => return Err(RubyErr::arg("output_buffer_limit must be positive")),
    };
    let mut input = rb::input_bytes(input)?;
    let release_gvl = input.len().saturating_add(limit) >= GVL_THRESHOLD;
    let output = with_mutex(
        &decompressor.state,
        release_gvl,
        &mut input,
        |state, input| state.process(input, limit),
    )?;
    rb::new_binary_string(&output)
}

/// Runs `read` on the decoder state, with the GVL.
fn decompressor_read<R>(rb_self: VALUE, read: fn(&DecoderState) -> R) -> RbResult<R> {
    let decompressor = decompressor_ref(rb_self)?;
    let mut input = rb::InputBytes::Owned(Vec::new());
    with_mutex(&decompressor.state, false, &mut input, |state, _| {
        state.check()?;
        Ok(read(state))
    })
}

unsafe extern "C" fn decompressor_new(class: VALUE, dictionary: VALUE) -> VALUE {
    rb::wrap(|| decompressor_new_impl(class, dictionary))
}

/// `_native_process(input, limit)`: `limit` is 0 for no limit.
unsafe extern "C" fn decompressor_process(rb_self: VALUE, input: VALUE, limit: VALUE) -> VALUE {
    rb::wrap(|| decompressor_process_impl(rb_self, input, limit))
}

/// Whether the stream is decoded and all of its output returned.
unsafe extern "C" fn decompressor_is_finished(rb_self: VALUE) -> VALUE {
    rb::wrap(|| decompressor_read(rb_self, |state| state.decoder.is_finished()).map(rb::bool_value))
}

/// Whether the decoder takes more input. False after the end of the stream,
/// and while it holds input or output from a call that reached its limit.
unsafe extern "C" fn decompressor_can_accept_more_data(rb_self: VALUE) -> VALUE {
    rb::wrap(|| {
        decompressor_read(rb_self, |state| {
            !state.decoder.is_finished() && state.tail.is_empty() && !state.pending
        })
        .map(rb::bool_value)
    })
}

/// Input after the end of the stream.
unsafe extern "C" fn decompressor_unused_data(rb_self: VALUE) -> VALUE {
    rb::wrap(|| {
        let unused = decompressor_read(rb_self, |state| state.unused.clone())?;
        rb::new_binary_string(&unused)
    })
}

// ---------- init ----------

/// # Safety
///
/// Ruby's extension loader calls this once, with the VM running.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn Init_burli_native() {
    rb::wrap(|| init().map(|()| rb::qnil()));
}

fn init() -> RbResult<()> {
    // SAFETY: called while the extension loads. Objects keep their state
    // behind their own lock, and no state is global.
    #[cfg(ruby_engine = "mri")]
    unsafe {
        rb_sys::rb_ext_ractor_safe(true);
    }

    let module = rb::define_module(c"Burli")?;
    // SAFETY: Ruby sets the class globals before it loads extensions.
    let (object, standard_error) = unsafe { (rb_sys::rb_cObject, rb_sys::rb_eStandardError) };
    let error_class = rb::define_class_under(module, c"Error", standard_error)?;
    ERROR
        .set(GlobalValue(error_class))
        .map_err(|_| RubyErr::runtime("burli_native loaded twice"))?;
    rb::define_singleton_method(module, c"_native_deflate", deflate as rb::Method5)?;
    rb::define_singleton_method(module, c"_native_inflate", inflate as rb::Method2)?;

    let compressor = rb::define_class_under(module, c"Compressor", object)?;
    rb::undef_alloc_func(compressor)?;
    rb::define_singleton_method(compressor, c"_native_new", compressor_new as rb::Method4)?;
    rb::define_method(compressor, c"process", compressor_process as rb::Method1)?;
    rb::define_method(compressor, c"flush", compressor_flush as rb::Method0)?;
    rb::define_method(compressor, c"finish", compressor_finish as rb::Method0)?;
    rb::define_method(
        compressor,
        c"finished?",
        compressor_is_finished as rb::Method0,
    )?;

    let decompressor = rb::define_class_under(module, c"Decompressor", object)?;
    rb::undef_alloc_func(decompressor)?;
    rb::define_singleton_method(
        decompressor,
        c"_native_new",
        decompressor_new as rb::Method1,
    )?;
    rb::define_method(
        decompressor,
        c"_native_process",
        decompressor_process as rb::Method2,
    )?;
    rb::define_method(
        decompressor,
        c"finished?",
        decompressor_is_finished as rb::Method0,
    )?;
    rb::define_method(
        decompressor,
        c"can_accept_more_data",
        decompressor_can_accept_more_data as rb::Method0,
    )?;
    rb::define_method(
        decompressor,
        c"unused_data",
        decompressor_unused_data as rb::Method0,
    )?;
    Ok(())
}
