# Changelog

## [Unreleased]

- Decode faster: two-level Huffman tables, chunked short match copies, and
  batched single-tree literals.
- Read compressed input through a 64-bit bit buffer refilled one word at a
  time.
- Speed up `paranoid` decoding with safe chunked match copies.
- Encode faster: flush `BitWriter` bytes with one fixed 8-byte append, and
  emit token literals in pairs. Output is unchanged.
- Fix corrupt output when the encoder switches match finders between
  meta-blocks. Streaming at q1 to q5 and one-shot inputs split into several
  meta-blocks could emit distance codes that pointed at the wrong distance.
- Benchmark small inputs as 64 distinct Silesia slices per size, from 512 B
  to 1 MiB. `small_decode.svg` also shows each decoder on its own output.

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
