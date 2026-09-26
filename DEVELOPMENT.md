# Development

Contributor workflow lives here so README can stay user-facing. Use this file
for build, test, release, fuzz, benchmark, and chart-generation commands.

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
(`.github/workflows/release.yml`). It opens or updates a release PR,
creates annotated tags after merge, publishes to crates.io, and creates
GitHub releases. Configuration lives in `release-plz.toml`.

Publishing uses crates.io trusted publishing through GitHub Actions OIDC.
Do not add a crates.io token secret.

### Steps

1. **Review the release-plz PR.** Verify semver bumps and dependency versions
   for every affected crate.

2. **Curate changelogs.** Insert a new `## [x.y.z]` section immediately below
   `## [Unreleased]` for each bumped crate and place its release notes there.
   Never rename or modify an existing versioned section.

3. **Bump the WASM package.** Update `jsr/deno.json` and
   `jsr/wasm/Cargo.toml`.

4. **Build and verify.** Run the JSR commands below and any additional release
   audit warranted by the changes.

5. **Merge the release PR after CI passes.** Monitor `release-plz` for crate
   publication and `.github/workflows/jsr.yml` for JSR publication.
   JSR rejects duplicate versions.

6. **Confirm publication.** Verify every intended crate version on crates.io
   and the new package version on each JavaScript registry. Run roundtrips
   against the published packages, including a bundled JSR roundtrip.

```bash
cd jsr
bash build.sh
deno task test
```

Use Deno 2.8.3 or newer for the bundled tests. They cover standalone bundles,
synchronous bytes, and initialization races. Bundled default initialization
runs without file or network permissions.

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
cargo fuzz run burli-decode-reuse
cargo fuzz run burli-roundtrip
```

## Benchmarks And Charts

Benchmarks are deliberate measurement work, not routine verification. The bench
crate is excluded from the workspace. Use `cargo run --manifest-path
bench/Cargo.toml`; `cargo bench` is not the supported path.

Before publishing new numbers or regenerating checked-in charts:

- run format, tests, and clippy first
- do not run two benchmarks or profilers at the same time
- stop on warnings, timeouts, or suspicious output
- record CPU, compiler flags, target features, corpus list, command, and ratio
- keep benchmark cache files append-only

### Smoke Check

Use short runs only to verify the harness and chart input path:

```bash
cargo run --manifest-path bench/Cargo.toml --example burli_bench --release -- \
  --impl burli --qualities 0,1,2,3,4,5 --chart-small-only --quick
cargo run --manifest-path bench/Cargo.toml --example burli_bench --release -- \
  --corpus silesia --impl burli --qualities 0,3,5 --quick
```

### Refresh Burli Chart Inputs

Routine chart refresh after Bürli code changes should only re-run `burli`:

```bash
cargo run --manifest-path bench/Cargo.toml --example burli_bench --release -- \
  --impl burli --qualities 0,1,2,3,4,5
cargo run --manifest-path bench/Cargo.toml --example burli_bench --release -- \
  --impl burli --qualities 0,1,2,3,4,5 --chart-small-only
cargo run --manifest-path bench/Cargo.toml --example burli_bench --release -- \
  --corpus silesia --impl burli --qualities 0,1,2,3,4,5
```

Re-run external baselines only when asked or when corpus or bench harness
changes:

```bash
cargo run --manifest-path bench/Cargo.toml --example burli_bench --release -- \
  --impl all --qualities 0,1,2,3,4,5
cargo run --manifest-path bench/Cargo.toml --example burli_bench --release -- \
  --impl all --qualities 0,1,2,3,4,5 --chart-small-only
cargo run --manifest-path bench/Cargo.toml --example burli_bench --release -- \
  --corpus silesia --impl all --qualities 0,1,2,3,4,5
```

The harness lazily downloads pinned corpus files into `bench/corpus/`.
JSONL results append under `~/.cache/burli/`, or under `BURLI_CACHE_DIR` when
set. Treat cache files as local, append-only data. Do not commit them.

Default timing is 30 ms per round, 3 rounds, 1 warmup. `--quick` uses one
30 ms round and no warmup. Use `--target-ms`, `--target-ns`, `--rounds`,
`--warmup`, or matching `BURLI_BENCH_*` env vars for focused work.

`--chart-small-only` restricts small-input runs to the files and sizes used by
the checked-in small charts: Silesia `dickens`, `nci`, `xml`, and `x-ray`,
from 512 B to 1 MiB. It avoids benchmarking every small slice of every corpus
file.

Each small input is up to 64 distinct consecutive slices of one size, and
each timed pass walks all of them. Repeating one slice lets the branch
predictor learn it and distorts results. Sizes and times in small rows are
totals per pass.

Default implementation set:

- `burli`
- `rust-brotli`
- `mbrotli`

`--impl all` also includes Google Brotli C through system `libbrotli`.
No CLI baseline.

### Render Charts

```bash
cargo run --manifest-path bench/Cargo.toml --bin burli_charts --release -- all
cargo run --manifest-path bench/Cargo.toml --bin burli_charts --release -- \
  scatter-silesia
```

`all` renders the web charts and both small-input charts. `scatter-silesia`
is separate. With no output dir, charts go under `doc/charts/<arch>/`.

Generated chart set:

- `summary.svg`
- `scatter.svg`
- `pipeline.svg`
- `matrix.svg`
- `small_encode.svg`
- `small_decode.svg`
- `scatter_silesia.svg`

`small_decode.svg` draws every decoder on the same Google Brotli C stream at
quality 5 (thick) and on its own quality 5 output (thin, from the small encode
rows). The thick lines need the Google-stream decode rows. The paranoid lines
need a paranoid small run:

```bash
cargo run --manifest-path bench/Cargo.toml --example burli_bench --release -- \
  --impl google-brotli-burli,google-brotli-rust-brotli,google-brotli-mbrotli \
  --qualities 5 --chart-small-only
cargo run --manifest-path bench/Cargo.toml --example burli_bench --release \
  --features paranoid -- --impl burli,google-brotli-burli --qualities 5 \
  --chart-small-only
```

Review SVG diffs before committing. Only commit chart files that were
intentionally refreshed.

Optional local hardware labels can live in ignored `.chart_hw`:

```text
prefix=Linux VM on a 2018 Mac Mini
cores=6
postfix=performance governor, turbo off
```

The chart tool reads `.chart_hw` from the current dir or parent dir. Env vars
`BURLI_HW_PREFIX`, `BURLI_HW_CORES`, `BURLI_HW_POSTFIX`, and
`BURLI_HW_EXTRAS` override or extend local detection.

Pipeline and matrix charts use stacked seconds/GB:

- compression CPU
- transfer at 100 MB/s
- decompression CPU

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
- compare `paranoid` builds separately when the default build gains unsafe code
