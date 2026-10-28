# Changelog

## [Unreleased]

## [0.1.0] - 2026-10-28

- Add the `burli` npm package, with the API of the Brotli functions of
  `node:zlib`: `brotliCompressSync`, `brotliDecompressSync`, `brotliCompress`,
  `brotliDecompress`, `createBrotliCompress`, `createBrotliDecompress`,
  `BrotliCompress`, `BrotliDecompress`, and `constants`. Qualities 6 to 11 run
  as 5. Callback functions and streams run on the libuv thread pool.
