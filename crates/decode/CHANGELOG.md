# Changelog

## [Unreleased]

## [0.3.1] - 2026-08-19

- Add bounded-memory Brotli stream validation.
- Preserve read-ahead bytes from `StreamDecoder` through `into_inner` errors.
- Improve decoder fast paths.

## [0.3.0] - 2026-08-18

- Remove the unused `simd` feature and `fearless_simd` dependency.

## [0.2.0] - 2026-08-17

- Rename the options API and deprecate `DecompressContext`.
- Return format errors instead of panicking on invalid untrusted context modes.
- Simplify Huffman validation and update the core dependency to `0.2.0`.

## [0.1.1] - 2026-08-16

- Add decoder options and owned raw-dictionary decode APIs.
- Add compressed fragment decoding support for `burli-cat`.

## [0.1.0] - 2026-08-16

- Initial public release of the burli Brotli decoder.
