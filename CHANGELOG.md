# Changelog

## [Unreleased]

- Add `PushDecoder`, which decodes a stream from input and output in
  pieces. It takes input only when it needs more, writes output as it
  decodes it, also within a meta-block, and never takes input past the
  stream end. `DecodeProgress` reports the input taken and output written,
  and `DecodeStatus` what the decoder needs next.
- `StreamDecoder` runs on `PushDecoder` and decodes in linear time, 5.8x to
  102x faster than 0.3.6 on the Silesia files. It decoded each meta-block
  again from its start for each 8 KiB of input, and copied the retained
  window for each meta-block: Silesia `mozilla` from rust-brotli q5 took
  23.3 s, against 0.25 s now and 0.20 s one-shot. It takes 1.02x to 1.45x
  the time of one-shot decoding, and returns output as soon as it decodes
  it, also within a meta-block.
- Add `StreamEncoder::get_ref` and `StreamEncoder::get_mut`.

## [0.3.6] - 2026-10-28

- All crates now enforce `#![forbid(unsafe_code)]`. The `paranoid` feature
  remains available and has no effect today.
- Decoding runs 34% to 68% faster than 0.3.5 on the benchmark corpora in
  aggregate, and every input decodes faster.
- q0 encodes in one pass, like Google's fast fragment encoder. On the
  benchmark files above 64 KiB its output is 8.6% to 27.5% smaller than
  with 0.3.5, at 0.51x to 1.08x the speed; only `citm-catalog.json` grows,
  by 5.8%. On the ten compressible Silesia files its ratio is 2.7% to 8.2%
  above Google Brotli q0. Low-compressibility input above 64 KiB gets a
  literal code where q0 stored it: Silesia `sao` and `x-ray` reach 1.067
  and 1.247 at about 1 GB/s.
- q1 encodes in two passes, with Huffman codes from the counts of its
  commands. On the benchmark files above 64 KiB its output is 1.0% to
  10.6% smaller than with 0.3.5, at 0.89x to 1.25x the speed, and smaller
  than Google Brotli q1 on each compressible one.
- On the compressible benchmark files above 64 KiB, q2 to q5 encode 1.20x
  to 1.67x faster than 0.3.5 at q2, 1.16x to 1.44x at q3, 1.04x to 1.52x at
  q4, and 1.03x to 1.39x at q5, with output within 0.1%. The exceptions
  are `citm-catalog.json` at q2 to q4, at 0.89x to 0.97x, and Silesia
  `osdb` at q5, at 0.96x. At q2 a meta-block no longer keeps a list of all
  its tokens, about 50 MB for 8 MiB of text.
- At q1 to q5, low-compressibility input above 64 KiB gets literal codes by
  the byte before, so ratio rises with quality. Silesia `sao` and `x-ray`
  go from 1.069 and 1.249 at every quality to 1.093 and 1.397 at q1 and
  1.282 and 1.475 at q5. q1 encodes them 1.06x to 1.10x as fast as 0.3.5,
  and q2 to q5 at 0.50x to 0.73x the speed.
- Bump the JSR/WASM package to `0.3.6`.

## [0.3.5] - 2026-09-27

- Size decoded output from the stream instead of the output limit:
  `decompress_with_limit`, `decompress_with_options`, and
  `Decompressor::decompress` returned a buffer with the whole limit as
  capacity when the limit was 16 MiB or less, even for a 2 KB output.
- Decode `decompress_into` in place when the output `Vec` is empty. It
  decoded into a temporary buffer and copied it, doubling peak memory.
- `decompress_into_slice` no longer allocates a temporary buffer the size of
  the caller's slice, and `validate` no longer reserves the whole window up
  front (4 MB for a 2 KB stream).
- Grow the output by exactly the last meta-block's length instead of
  doubling it.
- Bump the JSR/WASM package to `0.3.5`.

## [0.3.4] - 2026-09-26

- Add regression tests and a fuzz target for decoder reuse after malformed
  streams and across dictionary changes, output limits, and stream boundaries.
- Bump the JSR/WASM package to `0.3.4`.

## [0.3.3] - 2026-09-25

- Decode faster: two-level Huffman tables, chunked short match copies, and
  batched single-tree literals.
- Read compressed input through a 64-bit bit buffer refilled one word at a
  time.
- Speed up `paranoid` decoding with safe chunked match copies.
- Encode faster: flush `BitWriter` bytes with one fixed 8-byte append, and
  emit token literals in pairs. Output is unchanged.
- Keep q5 hash tables in the encoder workspace, so they keep their
  allocation across meta-blocks and reused `Compressor` calls.
- Fix corrupt output when the encoder switches match finders between
  meta-blocks. Streaming at q1 to q5 and one-shot inputs split into several
  meta-blocks could emit distance codes that pointed at the wrong distance.
- Benchmark small inputs as 64 distinct Silesia slices per size, from 512 B
  to 1 MiB. `small_decode.svg` also shows each decoder on its own output.
- Encode low-compressibility binary input much faster. q0 stores it at
  every size, not only from 64 KiB up. q1 to q5 store it below 32 KiB,
  16 KiB, 4 KiB, 1 KiB, and 512 B, and write literal-only meta-blocks
  from there. On Silesia `x-ray` this runs at 470 to 700 MB/s from 16 KiB
  up instead of 26 to 870 MB/s. Stored inputs grow by up to about 20%.
  Silesia `sao` grows by up to 13% at q4 and q5, where matching found
  some repeats.
- Build Huffman codes faster, which speeds up small inputs at every quality
  by up to 30%. Output is unchanged.
- Compress better when a Huffman code would be too deep, mostly at q0 and
  q1 on large text: Silesia `dickens` q0 shrinks by 13% and q1 by 9%.
- Encode q0 and q1 blocks faster through a new bit writer, and let q0
  probe fewer positions on inputs from 128 KiB up. q0 is 5% to 15% faster
  than before at a similar ratio on text and web files.

- Bump the JSR/WASM package to `0.3.3`.

## [0.3.2] - 2026-09-10

- Fix JSR package initialization after `deno bundle` by importing WASM and
  its generated bindings through the module graph.
- Reject invalid window bits in concat payload decoding.
- Decode streams with declared Huffman trees that their context maps do not use.
- Preserve following input after every streaming decoder termination shape.
- Make streaming encoder flushes emit pending input without ending the stream.
- Retain partial encoder output across write and flush errors so retries
  preserve the compressed stream.

- Bump the JSR/WASM package to `0.3.2`.

## [0.3.1] - 2026-08-19

- Add bounded-memory `burli::validate` for Brotli stream validation.
- Add RFC 7932 self-contained-part assembly through `burli-cat`.
- Preserve read-ahead bytes from `StreamDecoder` through `into_inner` errors.
- Hide internal module re-exports from the root API.
- Improve decoder fast paths and refresh benchmark charts.

## [0.3.0] - 2026-08-18

- Remove the unused SIMD feature and configuration API.
- Add the `@paddor/burli` JSR package for JavaScript and TypeScript.

## [0.2.0] - 2026-08-17

- Rename the options API to idiomatic Rust names and mark mode enums non-exhaustive.
- Improve encoder allocation behavior and share quality-level encoding logic.
- Return format errors for invalid decoder context modes.
- Add no-std compilation checks and refresh benchmark charts.

## [0.1.1] - 2026-08-16

- Add decoder options, streaming decode support, and owned raw-dictionary APIs.
- Add compressed fragment encoding and decoding support for `burli-cat`.
- Expand upstream conformance and API coverage.

## [0.1.0] - 2026-08-16

- Initial public release of the pure Rust Brotli codec.
