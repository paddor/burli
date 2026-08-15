# Bürli

Pure Rust Brotli codec. The decoder reads standard Brotli streams produced at
all normal quality levels. The encoder currently supports qualities 0 through
5.

## Why Bürli

**Memory safe by default.** Public API has no unsafe. Default-build unsafe is
kept in small primitive helpers. The `paranoid` feature forbids unsafe in all
Bürli crates.

**Fast pure-Rust q0..q5 encoder.** The encoder focuses on the web and transfer
part of Brotli's speed/ratio curve, with aggressive skip behavior for
low-compressibility input.

**Small API.** One-shot helpers for simple use, reusable contexts for hot loops,
and `std::io` streaming types when needed.

**No C dependency.** Google Brotli remains the compatibility and performance
baseline, but Bürli is pure Rust. Compared with `rust-brotli`, Bürli prioritizes
a smaller public surface, tighter safety boundary, and benchmark-visible encoder
policy.

## Performance

![Brotli pipeline benchmark](https://raw.githubusercontent.com/paddor/burli/main/doc/charts/x86_64/summary.svg)

<details>
<summary>x86_64 details</summary>

![per-file pipeline](https://raw.githubusercontent.com/paddor/burli/main/doc/charts/x86_64/pipeline.svg)
![encode speed vs compression ratio](https://raw.githubusercontent.com/paddor/burli/main/doc/charts/x86_64/scatter.svg)
![Silesia encode speed vs compression ratio](https://raw.githubusercontent.com/paddor/burli/main/doc/charts/x86_64/scatter_silesia.svg)
![per-file encode/decode matrix](https://raw.githubusercontent.com/paddor/burli/main/doc/charts/x86_64/matrix.svg)
![small input encode throughput](https://raw.githubusercontent.com/paddor/burli/main/doc/charts/x86_64/small_encode.svg)
![small input decode throughput](https://raw.githubusercontent.com/paddor/burli/main/doc/charts/x86_64/small_decode.svg)
</details>

## API

```rust
let compressed = burli::compress(input, 5)?;
let original = burli::decompress(&compressed)?;

let mut compressor = burli::Compressor::new(5)?;
let mut output = Vec::new();
compressor.compress_into(input, &mut output)?;

let mut decompressor = burli::Decompressor::new();
let mut decoded = Vec::new();
decompressor.decompress_into(&output, &mut decoded)?;
```

Streaming is available with `StreamEncoder` and `StreamDecoder` when the `std`
feature is enabled.
