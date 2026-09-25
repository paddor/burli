# Changelog

## [Unreleased]

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
