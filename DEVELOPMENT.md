# Development

## Build

```bash
cargo build
cargo build --no-default-features --features alloc
```

## Test

```bash
cargo test
cargo test --no-default-features --features alloc
cargo test --features paranoid
```

## Kani

Requires Kani:

```bash
cargo kani -p burli-core --output-format terse
```

Proofs are per-primitive with targeted bounds. Current harness covers
LSB-first bit extraction.

## Bench

Bench crate is excluded from the workspace:

```bash
cargo run --manifest-path bench/Cargo.toml --example burli_bench --release
cargo run --manifest-path bench/Cargo.toml --example burli_bench --release -- \
  --impl all --qualities 5
cargo run --manifest-path bench/Cargo.toml --example burli_bench --release -- \
  --impl all --qualities 0,1,2,3,4,5
cargo run --manifest-path bench/Cargo.toml --example burli_bench --release -- \
  --impl all --qualities 0,3,5
cargo run --manifest-path bench/Cargo.toml --example burli_bench --release -- \
  --impl all --qualities 0,1,2,3,4,5 \
  --files bootstrap-js,bootstrap-css,json-citm --small-only
```

The harness lazily downloads pinned web corpus files into `bench/corpus/`.
JSONL results append under `~/.cache/burli/`. Treat cache files as append-only.

Default implementation set:

- `burli`
- `rust-brotli`

`--impl all` also includes Google Brotli C through system `libbrotli`.
No CLI baseline.

## Charts

```bash
cargo run --manifest-path bench/Cargo.toml --bin burli_charts --release -- all
```

With no output dir, charts go under `doc/charts/<arch>/`.

Optional local hardware labels can live in ignored `.chart_hw`:

```text
prefix=Linux VM on a 2018 Mac Mini
cores=6
postfix=performance governor, turbo off
```

The chart tool reads `.chart_hw` from the current dir or parent dir. Env vars
`BURLI_HW_PREFIX`, `BURLI_HW_CORES`, `BURLI_HW_POSTFIX`, and
`BURLI_HW_EXTRAS` override or extend local detection.

Generated chart set:

- `scatter.svg`
- `summary.svg`
- `pipeline.svg`
- `matrix.svg`
- `small_encode.svg`
- `small_decode.svg`

Pipeline and matrix charts use stacked seconds/GB:

- compression CPU
- transfer at 100 MB/s
- decompression CPU

Routine chart refresh after burli code changes should only re-run `burli` and
`burli paranoid`. Re-run external baselines only when asked or when corpus or
bench harness changes.

Benchmark corpus:

- JavaScript
- CSS
- JSON
- HTML-like text
- tiny slices from 512 B through 1 MiB
- later: HTTP-body captures and malformed/adversarial decode corpus

Measurement rules:

- profile before optimizing
- record CPU, compiler flags, target features, corpus list, command, and ratio
- keep cache append-only
- compare scalar and SIMD paths separately
- compare default and `paranoid` builds separately
