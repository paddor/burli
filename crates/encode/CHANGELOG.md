# Changelog

## [Unreleased]

- Emit token literals in pairs in the q2 to q5 writers, and store length and
  distance extra bits in 32-bit fields. Output is unchanged.
- Track the decoder's distance ring across meta-blocks. Collectors that
  emit last-distance codes started from stale state after a meta-block
  written by another collector, which corrupted the decoded output.
- Detect low-compressibility input below 64 KiB with a branch-free sampler
  that rejects text on its first samples. q0 stores such input without
  building a workspace. q1 to q5 store it below a size that shrinks with
  the quality, from 32 KiB at q1 to 512 B at q5, and write 256 KiB
  literal-only meta-blocks above. Blocks of 64 KiB and more are sampled
  across their whole length for this route.
- Write static code-length headers in one pass.
- Build Huffman code lengths from packed frequency-symbol keys with a radix
  sort and a branch-free two-queue merge. Output is unchanged.
- Write literal-only meta-blocks four literals per bit write, and count
  bytes in eight interleaved tables.
- Limit Huffman code lengths by raising the smallest counts until the code
  fits, as Google's encoder does, instead of falling back to a balanced
  code. The balanced code spent about eight bits on every literal of large
  q0 and q1 blocks.
- Write q0 and q1 block bodies through `BitWriter::write_bits_with`, sized
  from the exact bit count, with literals packed four or three per write.
- Start q0's match skipping at 160 instead of 128 on its 32 Ki-entry
  table path.

## [0.3.2] - 2026-09-10

- Make streaming encoder flushes emit pending input without ending the stream.
- Retain partial output across stream write and flush errors for correct retries.

## [0.3.1] - 2026-08-19

- Improve encoder quality-level paths and streaming allocation behavior.

## [0.3.0] - 2026-08-18

- Remove the unused `simd` feature and `fearless_simd` dependency.

## [0.2.0] - 2026-08-17

- Rename the options API and deprecate `CompressContext`.
- Share quality-level encoding logic across encoder modules.
- Reduce per-chunk allocation in streaming encoding.
- Update the core dependency to `0.2.0`.

## [0.1.1] - 2026-08-16

- Add compressed fragment encoding support for `burli-cat`.

## [0.1.0] - 2026-08-16

- Initial public release of the burli Brotli encoder.
