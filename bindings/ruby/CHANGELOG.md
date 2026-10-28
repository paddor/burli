# Changelog

## [Unreleased]

## [0.1.0] - 2026-10-28

- Add the `burli` gem, with the API of the `brotli` gem: `Burli.deflate`,
  `Burli.inflate`, `Burli::Compressor`, `Burli::Decompressor` with
  `output_buffer_limit:`, `can_accept_more_data`, and `unused_data`,
  `Burli::Writer`, `Burli::Reader`, and `Burli::Error`. Qualities 6 to 11 run
  as 5. Raw dictionaries work for decompression.
