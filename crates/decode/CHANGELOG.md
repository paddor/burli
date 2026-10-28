# Changelog

## [Unreleased]

- Add `push::PushDecoder`, which decodes a stream from input and output in
  pieces. It takes input only when it needs more, writes output as it
  decodes it, also within a meta-block, and never takes input past the
  stream end. A compressed meta-block reads its header, then each prefix
  code, then runs a command loop that stops and goes on at checkpoints, so
  input split in any way decodes in linear time.
- `StreamDecoder` runs on `PushDecoder`. It decoded each meta-block again
  from its start for each 8 KiB of input and copied the retained window for
  each meta-block, which took 5.8x to 102x as long on the Silesia files.

## [0.3.5] - 2026-10-28

- Enforce `#![forbid(unsafe_code)]`. Backward copies, literal writes, and
  Huffman table lookups use only safe code. The `paranoid` feature has no
  effect today.
- Decode each compressed meta-block into zeroed room as a slice, so writes
  need no capacity checks. Short copies move whole 16- or 32-byte windows.
  Block counts and the current command and distance codes stay in locals
  between block switches. Literal contexts come from the 512-byte lookup
  table instead of a 64 KiB pair table.
- Keep the prefix codes of each alphabet in one buffer with 8-bit roots.
  Build tables from per-length symbol lists collected while reading code
  lengths, and size second-level tables before filling them. A meta-block
  with one literal code of mostly long codes also builds a 10-bit root.
  Literal context maps with one tree for every context use the single-code
  path.
- Keep the current literal block's context map, mode, and shared tree in
  locals, like the command and distance codes. One test sends a copy within
  the output and the window to the hot path; a cold path resolves dictionary
  references and rejects invalid distances.
- Write static dictionary words as fixed-size windows from a transform table
  with prefixes and suffixes inline, in one call from the command loop.
- Decoding runs 34% to 68% faster than 0.3.4 in aggregate.

## [0.3.4] - 2026-09-27

- Start one-shot output empty instead of reserving the output limit, when the
  limit is 16 MiB or less.
- Decode `decompress_into` in place when the output is empty.
- Start the `decompress_into_slice` scratch buffer and the `validate` window
  empty instead of reserving the slice length or the window size.
- Grow the output by exactly the last meta-block's length.

## [0.3.3] - 2026-09-25

- Use two-level Huffman lookup tables with a 10-bit root.
- Copy short backward matches in fixed 8- or 16-byte chunks.
- Decode single-tree literals three per bit refill.
- Refill the bit buffer once per command and distance symbol, and decode
  multi-block literals in per-block runs.
- Chunk short backward copies in the `paranoid` build with safe fixed-size
  appends.

## [0.3.2] - 2026-09-10

- Reject invalid window bits in the concat payload decoder.
- Preserve following input after every streaming decoder termination shape.
- Read all declared Huffman trees, including trees unused by the context maps.

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
