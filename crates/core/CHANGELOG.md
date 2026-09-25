# Changelog

## [Unreleased]

- Add hidden `BitReader::peek_bits_padded` for batched decoding.
- Buffer `BitReader` input in a 64-bit word. Add hidden `fill`, `refill`,
  `buffer`, and `buffered_bits` for decoder hot loops.
- Flush `BitWriter` bytes with one fixed 8-byte append instead of a
  variable-length copy.
- Add hidden `BitWriter::write_bits_batch_trusted_fits`, which writes many
  bit fields with the writer state kept in locals.

## [0.3.0] - 2026-08-18

- Remove the unused SIMD configuration API.

## [0.2.0] - 2026-08-17

- Rename the options API to idiomatic Rust names.
- Mark `Mode` non-exhaustive for future extension.
- Harden bit validation and sparse hash table bounds checks.

## [0.1.0] - 2026-08-16

- Initial public release of shared Brotli types and primitives for burli.
