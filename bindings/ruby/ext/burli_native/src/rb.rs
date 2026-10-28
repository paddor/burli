//! Helpers over rb-sys, after the ones in the zrip and lz4rip gems.
//!
//! Ruby calls that can raise run under `rb_protect`, so Ruby never jumps
//! over Rust frames. Entry points catch panics and raise Ruby errors only
//! once all Rust values are dropped.

use std::ffi::{CStr, CString, c_char, c_int, c_long, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr;

use rb_sys::{VALUE, rb_data_type_t};

pub type RbResult<T = VALUE> = Result<T, RubyErr>;

#[derive(Debug)]
pub enum RubyErr {
    /// A Ruby exception or throw that `rb_protect` caught.
    Jump(c_int),
    Error {
        class: VALUE,
        message: String,
    },
}

impl RubyErr {
    pub fn new(class: VALUE, message: impl Into<String>) -> Self {
        Self::Error {
            class,
            message: message.into(),
        }
    }

    pub fn arg(message: impl Into<String>) -> Self {
        // SAFETY: Ruby sets the class globals before it loads extensions.
        Self::new(unsafe { rb_sys::rb_eArgError }, message)
    }

    pub fn runtime(message: impl Into<String>) -> Self {
        // SAFETY: as above.
        Self::new(unsafe { rb_sys::rb_eRuntimeError }, message)
    }
}

/// Runs a method body; raises its error once `f` and its values are gone.
pub fn wrap<F>(f: F) -> VALUE
where
    F: FnOnce() -> RbResult<VALUE>,
{
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(Ok(value)) => value,
        Ok(Err(err)) => raise(err),
        Err(_) => raise(RubyErr::runtime("native Rust panic")),
    }
}

pub fn raise(err: RubyErr) -> ! {
    match err {
        // SAFETY: re-raises what `rb_protect` caught, with no Rust frames
        // left to skip.
        RubyErr::Jump(state) => unsafe { rb_sys::rb_jump_tag(state) },
        RubyErr::Error { class, message } => {
            let message = CString::new(message.replace('\0', "\\0")).unwrap_or_default();
            // SAFETY: as above.
            unsafe { rb_sys::rb_exc_raise(rb_sys::rb_exc_new_cstr(class, message.as_ptr())) }
        }
    }
}

struct ProtectData<F> {
    func: Option<F>,
    panicked: bool,
}

/// Runs `func`, which calls Ruby, and returns what it raised as an error.
pub fn protect<F>(func: F) -> RbResult<VALUE>
where
    F: FnOnce() -> VALUE,
{
    unsafe extern "C" fn call<F: FnOnce() -> VALUE>(arg: VALUE) -> VALUE {
        // SAFETY: `arg` is the `ProtectData` below, which outlives this call.
        let data = unsafe { &mut *(arg as *mut ProtectData<F>) };
        if let Some(Ok(value)) = data
            .func
            .take()
            .map(|func| catch_unwind(AssertUnwindSafe(func)))
        {
            return value;
        }
        data.panicked = true;
        qnil()
    }

    let mut data = ProtectData {
        func: Some(func),
        panicked: false,
    };
    let mut state = 0;
    // SAFETY: `call` gets a pointer to `data`, which lives across the call.
    let value =
        unsafe { rb_sys::rb_protect(Some(call::<F>), &raw mut data as VALUE, &raw mut state) };
    if state != 0 {
        Err(RubyErr::Jump(state))
    } else if data.panicked {
        Err(RubyErr::runtime("native Rust panic"))
    } else {
        Ok(value)
    }
}

#[cfg(ruby_engine = "mri")]
struct WithoutGvl<F, R> {
    func: Option<F>,
    output: Option<std::thread::Result<R>>,
}

/// Runs `func` without the GVL, so other threads run meanwhile. `func`
/// must not call Ruby.
///
/// `rb_thread_call_without_gvl2` neither raises nor checks interrupts, so an
/// interrupt cannot drop the output of work that changed native state. If
/// an interrupt is pending, `func` runs with the GVL, and Ruby handles the
/// interrupt when the method returns.
#[cfg(ruby_engine = "mri")]
pub fn without_gvl<F, R>(func: F) -> RbResult<R>
where
    F: FnOnce() -> R,
{
    unsafe extern "C" fn call<F: FnOnce() -> R, R>(data: *mut c_void) -> *mut c_void {
        // SAFETY: `data` is the `WithoutGvl` below, which outlives this call.
        let data = unsafe { &mut *data.cast::<WithoutGvl<F, R>>() };
        if let Some(func) = data.func.take() {
            // Panics must not unwind into Ruby.
            data.output = Some(catch_unwind(AssertUnwindSafe(func)));
        }
        ptr::null_mut()
    }

    let mut data = WithoutGvl {
        func: Some(func),
        output: None,
    };
    // SAFETY: `call` touches only `data`, which lives across the call.
    unsafe {
        rb_sys::rb_thread_call_without_gvl2(
            Some(call::<F, R>),
            (&raw mut data).cast(),
            None,
            ptr::null_mut(),
        );
    }
    let output = match (data.output, data.func) {
        (Some(output), _) => output,
        (None, Some(func)) => catch_unwind(AssertUnwindSafe(func)),
        (None, None) => return Err(RubyErr::runtime("without-GVL work did not run")),
    };
    output.map_err(|_| RubyErr::runtime("native Rust panic"))
}

/// Runs `func` without the GVL when `release_gvl`. `TruffleRuby` has no GVL
/// to release.
pub fn maybe_without_gvl<F, R>(release_gvl: bool, func: F) -> RbResult<R>
where
    F: FnOnce() -> R,
{
    #[cfg(ruby_engine = "mri")]
    if release_gvl {
        return without_gvl(func);
    }
    let _ = release_gvl;
    catch_unwind(AssertUnwindSafe(func)).map_err(|_| RubyErr::runtime("native Rust panic"))
}

pub const fn qnil() -> VALUE {
    rb_sys::ruby_special_consts::RUBY_Qnil as VALUE
}

pub const fn qtrue() -> VALUE {
    rb_sys::ruby_special_consts::RUBY_Qtrue as VALUE
}

pub const fn qfalse() -> VALUE {
    rb_sys::ruby_special_consts::RUBY_Qfalse as VALUE
}

pub const fn bool_value(value: bool) -> VALUE {
    if value { qtrue() } else { qfalse() }
}

/// A new ASCII-8BIT string with `bytes`.
pub fn new_binary_string(bytes: &[u8]) -> RbResult<VALUE> {
    let len = c_long::try_from(bytes.len()).map_err(|_| RubyErr::arg("length too large"))?;
    // SAFETY: `bytes` is valid for `len` bytes; Ruby copies them.
    protect(|| unsafe { rb_sys::rb_str_new(bytes.as_ptr().cast::<c_char>(), len) })
}

/// `value` as a String, through `to_str`.
fn string_value(value: VALUE) -> RbResult<VALUE> {
    // SAFETY: `rb_str_to_str` takes any object and raises on failure.
    protect(|| unsafe { rb_sys::rb_str_to_str(value) })
}

/// The bytes of `string`, valid while Ruby cannot change or free it.
///
/// # Safety
///
/// `string` must be a String.
unsafe fn string_bytes<'a>(string: VALUE) -> &'a [u8] {
    // SAFETY: the caller passes a String.
    let (data, len) = unsafe { (rb_sys::RSTRING_PTR(string), rb_sys::RSTRING_LEN(string)) };
    match usize::try_from(len) {
        Ok(len) if len > 0 && !data.is_null() => {
            // SAFETY: a String holds `len` bytes at `data`.
            unsafe { std::slice::from_raw_parts(data.cast::<u8>(), len) }
        }
        _ => &[],
    }
}

/// A copy of the bytes of `value`, through `to_str`.
pub fn value_to_bytes(value: VALUE) -> RbResult<Vec<u8>> {
    let string = string_value(value)?;
    // SAFETY: `string` is a String, read before Ruby runs again.
    Ok(unsafe { string_bytes(string) }.to_vec())
}

pub fn value_to_option_bytes(value: VALUE) -> RbResult<Option<Vec<u8>>> {
    if value == qnil() {
        return Ok(None);
    }
    value_to_bytes(value).map(Some)
}

/// Input bytes, borrowed from a String where Ruby allows it.
pub enum InputBytes {
    #[cfg(ruby_engine = "mri")]
    Borrowed(BorrowedString),
    Owned(Vec<u8>),
}

impl InputBytes {
    pub fn as_slice(&self) -> &[u8] {
        match self {
            #[cfg(ruby_engine = "mri")]
            Self::Borrowed(string) => string.as_slice(),
            Self::Owned(bytes) => bytes,
        }
    }

    pub fn len(&self) -> usize {
        self.as_slice().len()
    }

    /// Keeps other threads from changing the bytes while the GVL is
    /// released. Copies them when the String cannot be locked, as while
    /// another thread works on it.
    pub fn lock(&mut self) {
        match self {
            #[cfg(ruby_engine = "mri")]
            Self::Borrowed(string) => {
                if !string.lock() {
                    let bytes = string.as_slice().to_vec();
                    *self = Self::Owned(bytes);
                }
            }
            Self::Owned(_) => {}
        }
    }
}

#[cfg(ruby_engine = "mri")]
pub struct BorrowedString {
    value: VALUE,
    locked: bool,
}

#[cfg(ruby_engine = "mri")]
impl BorrowedString {
    fn as_slice(&self) -> &[u8] {
        // SAFETY: `value` is a String. While the GVL is held, only this
        // thread runs; without it, the String is frozen or locked.
        unsafe { string_bytes(self.value) }
    }

    /// Whether the String is frozen or now locked.
    fn lock(&mut self) -> bool {
        // SAFETY: `value` is a String.
        if self.locked || unsafe { rb_sys::rb_obj_frozen_p(self.value) } == qtrue() {
            return true;
        }
        // SAFETY: raises if the String is already locked, which `protect`
        // catches.
        if protect(|| unsafe { rb_sys::rb_str_locktmp(self.value) }).is_err() {
            // SAFETY: drops the caught error, so it does not show as `$!`.
            unsafe { rb_sys::rb_set_errinfo(qnil()) };
            return false;
        }
        self.locked = true;
        true
    }
}

#[cfg(ruby_engine = "mri")]
impl Drop for BorrowedString {
    fn drop(&mut self) {
        if self.locked {
            // SAFETY: undoes the `rb_str_locktmp` above.
            unsafe { rb_sys::rb_str_unlocktmp(self.value) };
        }
        // Keeps the String alive until here.
        let _ = rb_sys::rb_gc_guard!(self.value);
    }
}

/// The bytes of `value`, through `to_str`. `TruffleRuby` gets a copy.
pub fn input_bytes(value: VALUE) -> RbResult<InputBytes> {
    #[cfg(ruby_engine = "mri")]
    {
        let value = string_value(value)?;
        Ok(InputBytes::Borrowed(BorrowedString {
            value,
            locked: false,
        }))
    }
    #[cfg(not(ruby_engine = "mri"))]
    {
        value_to_bytes(value).map(InputBytes::Owned)
    }
}

pub fn value_to_i64(value: VALUE) -> RbResult<i64> {
    let mut out = 0;
    // SAFETY: raises for non-Integers, which `protect` catches.
    protect(|| {
        out = unsafe { rb_sys::rb_num2ll(value) };
        qnil()
    })?;
    Ok(out)
}

/// Wraps `value` in a new object of `class`; drops it if that fails.
///
/// # Safety
///
/// `data_type` must free a `Box<T>`.
pub unsafe fn wrap_typed_data<T>(
    class: VALUE,
    value: Box<T>,
    data_type: *const rb_data_type_t,
) -> RbResult<VALUE> {
    let raw = Box::into_raw(value);
    // SAFETY: Ruby owns `raw` once the object exists.
    let wrapped = protect(|| unsafe {
        rb_sys::rb_data_typed_object_wrap(class, raw.cast::<c_void>(), data_type)
    });
    if wrapped.is_err() {
        // SAFETY: no object took `raw`.
        drop(unsafe { Box::from_raw(raw) });
    }
    wrapped
}

/// The `T` that `value` wraps.
///
/// # Safety
///
/// `data_type` must wrap a `T`. The reference must not outlive `value`.
pub unsafe fn typed_data_ref<'a, T>(
    value: VALUE,
    data_type: *const rb_data_type_t,
) -> RbResult<&'a T> {
    let mut data = ptr::null_mut();
    // SAFETY: raises a TypeError for other objects, which `protect` catches.
    protect(|| {
        data = unsafe { rb_sys::rb_check_typeddata(value, data_type) };
        qnil()
    })?;
    if data.is_null() {
        return Err(RubyErr::runtime("object has no native data"));
    }
    // SAFETY: the caller's contract above.
    Ok(unsafe { &*data.cast::<T>() })
}

pub fn define_module(name: &CStr) -> RbResult<VALUE> {
    // SAFETY: `name` is a C string.
    protect(|| unsafe { rb_sys::rb_define_module(name.as_ptr()) })
}

pub fn define_class_under(outer: VALUE, name: &CStr, superclass: VALUE) -> RbResult<VALUE> {
    // SAFETY: as above.
    protect(|| unsafe { rb_sys::rb_define_class_under(outer, name.as_ptr(), superclass) })
}

/// Classes whose objects only native constructors make.
pub fn undef_alloc_func(class: VALUE) -> RbResult<()> {
    // SAFETY: `class` is a class.
    protect(|| {
        unsafe { rb_sys::rb_undef_alloc_func(class) };
        qnil()
    })?;
    Ok(())
}

/// A method body that takes the receiver and `ARITY` arguments.
pub trait Method {
    const ARITY: c_int;
    fn erase(self) -> unsafe extern "C" fn() -> VALUE;
}

macro_rules! method_arity {
    ($arity:literal $(, $arg:ty)*) => {
        impl Method for unsafe extern "C" fn(VALUE $(, $arg)*) -> VALUE {
            const ARITY: c_int = $arity;

            fn erase(self) -> unsafe extern "C" fn() -> VALUE {
                // SAFETY: Ruby calls a method of arity N with the receiver
                // and N arguments, as this type takes.
                unsafe { std::mem::transmute::<Self, unsafe extern "C" fn() -> VALUE>(self) }
            }
        }
    };
}

method_arity!(0);
method_arity!(1, VALUE);
method_arity!(2, VALUE, VALUE);
method_arity!(3, VALUE, VALUE, VALUE);
method_arity!(4, VALUE, VALUE, VALUE, VALUE);
method_arity!(5, VALUE, VALUE, VALUE, VALUE, VALUE);

pub fn define_method<M: Method>(class: VALUE, name: &CStr, func: M) -> RbResult<()> {
    let func = func.erase();
    // SAFETY: `func` takes the arguments `M::ARITY` says.
    protect(|| {
        unsafe { rb_sys::rb_define_method(class, name.as_ptr(), Some(func), M::ARITY) };
        qnil()
    })?;
    Ok(())
}

pub fn define_singleton_method<M: Method>(object: VALUE, name: &CStr, func: M) -> RbResult<()> {
    let func = func.erase();
    // SAFETY: as above.
    protect(|| {
        unsafe { rb_sys::rb_define_singleton_method(object, name.as_ptr(), Some(func), M::ARITY) };
        qnil()
    })?;
    Ok(())
}

/// Method body types, for `define_method`.
pub type Method0 = unsafe extern "C" fn(VALUE) -> VALUE;
pub type Method1 = unsafe extern "C" fn(VALUE, VALUE) -> VALUE;
pub type Method2 = unsafe extern "C" fn(VALUE, VALUE, VALUE) -> VALUE;
pub type Method4 = unsafe extern "C" fn(VALUE, VALUE, VALUE, VALUE, VALUE) -> VALUE;
pub type Method5 = unsafe extern "C" fn(VALUE, VALUE, VALUE, VALUE, VALUE, VALUE) -> VALUE;
