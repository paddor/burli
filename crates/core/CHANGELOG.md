# Changelog

## [Unreleased]

- Enforce `#![forbid(unsafe_code)]`. The `paranoid` feature has no effect
  today.
- `BitReader` keeps the unread input as a slice, so a refill tests one
  length. Add hidden `read_short_bits` for fields of up to 24 bits.
- Refill `BitReader` with a mask and an OR instead of a clamp and an add.
- `BurliError` has a `u64` tag. With a byte tag, decoder error paths kept
  stale values live in the command loop.

## [0.3.1] - 2026-09-25

- Add hidden `BitReader::peek_bits_padded` for batched decoding.
- Buffer `BitReader` input in a 64-bit word. Add hidden `fill`, `refill`,
  `buffer`, and `buffered_bits` for decoder hot loops.
- Flush `BitWriter` bytes with one fixed 8-byte append instead of a
  variable-length copy.
- Add hidden `BitWriter::write_bits_batch_trusted_fits`, which writes many
  bit fields with the writer state kept in locals.
- Add hidden `BitWriter::write_bits_with` and `BitSink`, which write into
  output reserved up front.

## [0.3.0] - 2026-08-18

- Remove the unused SIMD configuration API.

## [0.2.0] - 2026-08-17

- Rename the options API to idiomatic Rust names.
- Mark `Mode` non-exhaustive for future extension.
- Harden bit validation and sparse hash table bounds checks.

## [0.1.0] - 2026-08-16

- Initial public release of shared Brotli types and primitives for burli.
