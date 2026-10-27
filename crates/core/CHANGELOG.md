# Changelog

## [Unreleased]

## [0.3.2] - 2026-10-28

- Enforce `#![forbid(unsafe_code)]`. The `paranoid` feature has no effect
  today.
- `BitReader` keeps the unread input as a slice, so a refill tests one
  length. Add hidden `read_short_bits` for fields of up to 24 bits.
- Refill `BitReader` with a mask and an OR instead of a clamp and an add.
- `BurliError` has a `u64` tag. With a byte tag, decoder error paths kept
  stale values live in the command loop.
- `BitSink` ORs each write into the partial byte in memory and keeps only
  the bit position, like Google's `BrotliWriteBits`. Writer loops keep two
  fewer registers live.
- Add hidden `BitWriter::write_bits_up_to` and `BitSink::has_room` for
  writers that know only a bound on their output, and hidden
  `BitWriter::mark`, `BitWriter::rewind`, and `BitMark` to drop what was
  written since a mark.
- `BitWriter::finish_into` hands over its buffer when the output `Vec` has
  no capacity, instead of copying it.
- Add hidden `BitWriter::write_bits_raw`, which lends zeroed room for a
  bound of bits to writers that keep their own bit buffer.

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
