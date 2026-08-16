# Development

## Build

```bash
cargo build --workspace
cargo build --workspace --features paranoid
cargo build -p burli --no-default-features --features alloc
```

## Test

```bash
cargo nextest run --workspace
cargo test --workspace --features paranoid
cargo test -p burli --no-default-features --features alloc
cargo clippy --workspace --all-targets -- -D warnings
```

Long decoder conformance tests are ignored by default:

```bash
cargo nextest run --profile conformance --release --test burli-google-conformance \
  --run-ignored ignored-only
cargo nextest run --profile conformance --release --test burli-c-brotli \
  --run-ignored ignored-only
```

Focused upstream case:

```bash
BURLI_GOOGLE_BROTLI_CASE=alice29.txt.compressed \
  cargo nextest run --profile conformance --release --test burli-google-conformance \
  --run-ignored ignored-only
```

Exhaustive byte-fragmented upstream stream soak:

```bash
BURLI_GOOGLE_BROTLI_FRAGMENTED_EXHAUSTIVE=1 \
  cargo nextest run --profile soak --release --test burli-google-conformance \
  --run-ignored ignored-only
```

## Releasing

`release-plz` runs on every push to `main`
(`.github/workflows/release-plz.yml`). It opens or updates a release PR,
creates annotated tags after merge, publishes to crates.io, and creates
GitHub releases. Configuration lives in `release-plz.toml`.

Publishing uses crates.io trusted publishing through GitHub Actions OIDC. Do
not add a crates.io token secret unless trusted publishing cannot be used.

### Steps

1. **Review the release-plz PR.** Verify semver bumps and crate order.

2. **Run any needed release audit.** Use the Kani and fuzz commands below when
   the release risk warrants an extended audit.

3. **Merge the release PR.** release-plz tags and publishes to crates.io
   automatically.

## Kani

Requires Kani:

```bash
cargo kani -p burli-core --output-format terse
cargo kani -p burli-encode --output-format terse
```

Proofs are per-primitive with targeted bounds. Current harness covers
LSB-first bit extraction, peek/drop reader invariants, and literal encoder
insert command mapping.

## Fuzz

Requires cargo-fuzz:

```bash
cargo fuzz run burli-decode
cargo fuzz run burli-roundtrip
```

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
cargo run --manifest-path bench/Cargo.toml --example burli_bench --release -- \
  --impl burli --qualities 0,1,2,3,4,5 --chart-small-only --quick
cargo run --manifest-path bench/Cargo.toml --example burli_bench --release -- \
  --corpus silesia --impl all --qualities 0,1,2,3,4,5
```

The harness lazily downloads pinned corpus files into `bench/corpus/`.
JSONL results append under `~/.cache/burli/`. Treat cache files as append-only.
Default timing is 30 ms per round, 3 rounds, 1 warmup. `--quick` uses one
30 ms round and no warmup for smoke checks. Use `--target-ms`, `--target-ns`,
`--rounds`, `--warmup`, or matching `BURLI_BENCH_*` env vars for focused
work.

`--chart-small-only` restricts small-input runs to the files and sizes used by
the checked-in small charts. It avoids benchmarking every small slice of every
corpus file.

Default implementation set:

- `burli`
- `rust-brotli`

`--impl all` also includes Google Brotli C through system `libbrotli`.
No CLI baseline.

## Charts

```bash
cargo run --manifest-path bench/Cargo.toml --bin burli_charts --release -- all
cargo run --manifest-path bench/Cargo.toml --bin burli_charts --release -- \
  scatter-silesia
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
- `scatter_silesia.svg`
- `small_encode.svg`
- `small_decode.svg`

Pipeline and matrix charts use stacked seconds/GB:

- compression CPU
- transfer at 100 MB/s
- decompression CPU

Routine chart refresh after burli code changes should only re-run `burli`.
Re-run external baselines only when asked or when corpus or bench harness
changes.

Benchmark corpus:

- JavaScript
- CSS
- JSON
- HTML-like text
- Silesia text/code, medium-compressibility data, and low-compressibility data
- tiny slices from 512 B through 1 MiB
- later: HTTP-body captures and malformed/adversarial decode corpus

Measurement rules:

- profile before optimizing
- record CPU, compiler flags, target features, corpus list, command, and ratio
- keep cache append-only
- compare scalar and SIMD paths separately
- compare `paranoid` builds separately when the default build gains unsafe code
