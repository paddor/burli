# Changelog

## [Unreleased]

## [0.1.1] - 2026-10-28

- Compression takes at most 120 KiB of stack, down from 608 KiB.

## [0.1.0] - 2026-10-28

- Add the `burli` Python package, with the API of Google's `brotli` module:
  `compress`, `decompress`, `Compressor`, `Decompressor` with
  `output_buffer_limit` and `can_accept_more_data()`, `error`, and the
  `MODE_*` constants. Qualities 6 to 11 run as 5. Wheels use the stable ABI
  for Python 3.9 and later.
