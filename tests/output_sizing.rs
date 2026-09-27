//! Decoding sizes its buffers from what the stream produces, not from the
//! caller's limit, the caller's slice, or the stream's window.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

/// Tracks live heap bytes per thread, so tests running in parallel do not
/// see each other's allocations.
struct Counting;

thread_local! {
    static LIVE: Cell<isize> = const { Cell::new(0) };
    static PEAK: Cell<isize> = const { Cell::new(0) };
}

fn track(delta: isize) {
    let _ = LIVE.try_with(|live| {
        let now = live.get() + delta;
        live.set(now);
        let _ = PEAK.try_with(|peak| peak.set(peak.get().max(now)));
    });
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        track(layout.size() as isize);
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        track(layout.size() as isize);
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        track(-(layout.size() as isize));
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        track(new_size as isize - layout.size() as isize);
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

/// Returns `f`'s result and the most heap memory it held at once.
fn peak_heap<T>(f: impl FnOnce() -> T) -> (T, usize) {
    let base = LIVE.with(Cell::get);
    PEAK.with(|peak| peak.set(base));
    let result = f();
    (result, (PEAK.with(Cell::get) - base) as usize)
}

fn text(len: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(len);
    let mut i = 0u32;
    while out.len() < len {
        out.extend_from_slice(format!("level=INFO service=ingest event={i}\n").as_bytes());
        i += 1;
    }
    out.truncate(len);
    out
}

/// Encodes with Google's encoder at quality 5, which writes one meta-block
/// for inputs of this size.
fn google_brotli(data: &[u8]) -> Vec<u8> {
    let params = rust_brotli::enc::BrotliEncoderParams {
        quality: 5,
        ..Default::default()
    };
    let mut out = Vec::new();
    rust_brotli::BrotliCompress(&mut &data[..], &mut out, &params).unwrap();
    out
}

const LIMIT: usize = 16 << 20;

#[test]
fn output_limit_does_not_size_the_output() {
    let data = text(2_000);
    let stream = burli::compress(&data, 1).unwrap();

    let output = burli::decompress_with_limit(&stream, LIMIT).unwrap();
    assert_eq!(output, data);
    assert!(output.capacity() < 4 * 1024, "{} bytes", output.capacity());

    let output = burli::Decompressor::with_limit(LIMIT)
        .decompress(&stream)
        .unwrap();
    assert_eq!(output, data);
    assert!(output.capacity() < 4 * 1024, "{} bytes", output.capacity());
}

#[test]
fn decompress_into_an_empty_vec_decodes_in_place() {
    let data = text(256 * 1024);
    let stream = google_brotli(&data);
    let (output, peak) = peak_heap(|| {
        let mut output = Vec::new();
        burli::decompress_into(&stream, &mut output).unwrap();
        output
    });
    assert_eq!(output, data);
    // One output buffer and the decoder's tables, not a second copy.
    assert!(peak < data.len() * 3 / 2, "peak heap {peak} bytes");
}

#[test]
fn decompress_into_leaves_the_output_empty_on_error() {
    let data = text(256 * 1024);
    let stream = google_brotli(&data);
    let mut output = Vec::new();
    assert!(burli::decompress_into(&stream[..stream.len() - 1], &mut output).is_err());
    assert!(output.is_empty());
}

#[test]
fn decompress_into_slice_does_not_size_from_the_slice() {
    let data = text(2_000);
    let stream = burli::compress(&data, 1).unwrap();
    let mut slice = vec![0u8; LIMIT];
    let (written, peak) = peak_heap(|| burli::decompress_into_slice(&stream, &mut slice).unwrap());
    assert_eq!(&slice[..written], &data[..]);
    assert!(peak < 64 * 1024, "peak heap {peak} bytes");
}

#[test]
fn validate_does_not_reserve_the_window() {
    let data = text(2_000);
    let stream = burli::compress(&data, 1).unwrap();
    let ((), peak) = peak_heap(|| burli::validate(&stream).unwrap());
    assert!(peak < 64 * 1024, "peak heap {peak} bytes");
}
