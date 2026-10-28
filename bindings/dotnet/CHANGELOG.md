# Changelog

## [Unreleased]

- Compression no longer overflows the stack of .NET 8 threads on macOS,
  which have 512 KiB. Quality 2 took up to 608 KiB; compression now takes
  at most 120 KiB.

## [0.1.0] - 2026-10-28

- Add the `Burli` NuGet package, with the API of the Brotli types of
  `System.IO.Compression`: `BrotliStream`, `BrotliEncoder`, `BrotliDecoder`,
  and `BrotliCompressionOptions`. Qualities 6 to 11 run as 5.
