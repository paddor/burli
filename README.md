# Bürli

Pure Rust Brotli codec. The decoder reads standard Brotli streams produced at
all normal quality levels. The encoder currently supports qualities 0 through
5.

## Why Bürli

**Fast q0..q5 encoder.** Covers the transfer-oriented part of Brotli's
speed/ratio curve, with aggressive skip behavior for low-compressibility input.

**Memory safety.** Public API has no unsafe. Unsafe is limited to small
primitive helpers in the default build. The `paranoid` feature forbids unsafe in
all Bürli crates.

**Small API.** One-shot helpers for simple use, caller-buffer variants for tight
loops, reusable contexts for repeated work, and `std::io` streaming wrappers
when needed.

## Performance

![Brotli pipeline benchmark](https://raw.githubusercontent.com/paddor/burli/main/doc/charts/x86_64/summary.svg)

<details>
<summary>x86_64 details (pipeline, scatter, matrix, small inputs)</summary>

![per-file pipeline](https://raw.githubusercontent.com/paddor/burli/main/doc/charts/x86_64/pipeline.svg)
![encode speed vs compression ratio](https://raw.githubusercontent.com/paddor/burli/main/doc/charts/x86_64/scatter.svg)
![Silesia encode speed vs compression ratio](https://raw.githubusercontent.com/paddor/burli/main/doc/charts/x86_64/scatter_silesia.svg)
![per-file encode/decode matrix](https://raw.githubusercontent.com/paddor/burli/main/doc/charts/x86_64/matrix.svg)
![small input encode throughput](https://raw.githubusercontent.com/paddor/burli/main/doc/charts/x86_64/small_encode.svg)
![small input decode throughput](https://raw.githubusercontent.com/paddor/burli/main/doc/charts/x86_64/small_decode.svg)
</details>

## API

```rust
// One-shot (allocating)
let compressed = burli::compress(input, 5)?;
let original   = burli::decompress(&compressed)?;

// One-shot into caller buffer
let n = burli::compress_into(input, &mut output_buf, 5)?;
burli::decompress_into(&compressed, &mut output_vec)?;

// Reusable context
let mut compressor = burli::Compressor::new(5)?;
let compressed = compressor.compress(input)?;

let mut decompressor = burli::Decompressor::new();
let original = decompressor.decompress(&compressed)?;
```

### Streaming

```rust
use std::io::{Read, Write};

let mut enc = burli::StreamEncoder::new(Vec::new(), 5)?;
enc.write_all(input)?;
let compressed = enc.finish()?;

let mut dec = burli::StreamDecoder::new(&compressed[..]);
let mut decoded = Vec::new();
dec.read_to_end(&mut decoded)?;
```

### Raw Dictionaries

Raw LZ77 prefix dictionaries are decode-only for now:

```rust
let original = burli::decompress_with_raw_dictionary(&compressed, dictionary)?;
```

## Safety

[SAFETY.md](SAFETY.md) documents the unsafe boundary and Brotli bug classes
that Bürli is designed to prevent.

Bounded decompression is first-class: use `decompress_with_options`,
`Decompressor::with_options`, `StreamDecoder::with_options`, or
`decompress_into_slice` for untrusted input. Plain `decompress()` has no
practical output cap.

Current safety checks include Kani coverage for low-level primitives and more
than 6 hours of `libFuzzer` coverage across decode, encode/decode round trips,
corruption cases, streaming, and C Brotli cross-checks.

## Design

[DESIGN.md](DESIGN.md) covers the implemented encode/decode pipeline, bit I/O,
Huffman tables, backward copies, dictionaries, streaming, and quality policy.

## Levels

Bürli's encoder covers the fast end of Brotli. q0 favors throughput and may
store low-compressibility blocks. q1 through q5 spend progressively more work on
matching and entropy coding for better ratios.

The current encoder stops at q5. Brotli q6 through q11 spend much more CPU for
ratios that usually matter more for archival storage than transfer pipelines.
All implemented qualities produce standard Brotli streams decoded by the same
decoder.
