# Changelog

## [Unreleased]

- Enforce `#![forbid(unsafe_code)]`. The match scanner reads words through
  checked slices instead of unaligned pointer reads. The `paranoid` feature
  has no effect today.
- Build canonical code maps without a counter update for unused symbols.
  Output is unchanged.
- q0 writes meta-blocks of up to 64 KiB in one pass, like Google's fast
  fragment encoder. The literal code comes from the input's byte counts,
  and commands and distances use fixed codes chosen by input size: a flat
  code up to 4 KiB and two tuned codes up to 16 KiB and 64 KiB. q0 output
  changes for these inputs.
- Build the one-pass literal code from the byte values that occur only,
  with an in-place Huffman pass and a table for the zero runs in its
  header.
- One-shot q0 calls on inputs of up to 64 KiB skip the encoder workspace,
  which took several KiB to clear, and return the writer's buffer without
  a copy.
- Remove the q0 collectors and writers for meta-blocks below 64 KiB.
- q0 writes longer inputs in one pass too, in meta-blocks of 256 KiB. Their
  literal codes come from a sample of each block, and their command and
  distance codes from the counts of the block before. Runs of incompressible
  blocks go out as one stored meta-block. This needs a window of at least
  256 KiB; shorter windows get one-pass meta-blocks of up to 64 KiB. q0
  output changes for these inputs.
- q1 compresses inputs of up to 64 KiB in two passes: the matcher collects
  commands with combined insert-and-copy symbols and counts their symbols,
  and each alphabet then gets a Huffman code. q1 output changes for these
  inputs.
- Low-compressibility blocks of 64 KiB and more get literal codes by the
  top six bits of the byte before at q2 to q5, with up to 2, 4, 16, and 64
  codes merged from 64 context histograms. On Silesia `sao` and `x-ray`,
  q2 to q5 reach ratios of 1.093 to 1.282 and 1.401 to 1.475, where q1 to
  q5 all reached 1.069 and 1.249.
- The q2 to q5 collectors push tokens straight into the writer, which packs
  each into a 24-byte command, instead of collecting 48-byte tokens for the
  whole meta-block first. At q2, an 8 MiB meta-block of Silesia `dickens`
  no longer keeps about 50 MB of tokens. Output is unchanged.
- Build the q2 to q5 Huffman codes with the q0 and q1 code builder, which
  works only on the symbols that occur and tunes the code-length code.
  Output is up to 0.46% smaller on short inputs and never larger on
  Silesia.
- With these two writer changes, the ten compressible Silesia files encode
  1.22x to 1.69x faster at q2, 1.21x to 1.46x at q3, 1.13x to 1.40x at q4,
  and 1.07x to 1.28x at q5, and inputs of 512 B up to 1.7x faster at q3
  to q5.
- One-shot calls at every quality return the writer's buffer without a
  copy.
- Remove the q0 two-pass collectors for longer inputs and the batch writer
  that prepared every token before writing.
- q1 compresses inputs above 64 KiB with a matcher like q0's, which
  pushes its commands into the q2 to q5 token writer, so they take
  combined insert-and-copy symbols and Huffman codes per meta-block. Text
  takes matches of 6 bytes and more with a table of 2^16 positions, and
  input with more than 1 in 12 sampled bytes zero takes 5-byte matches
  with 2^15. On the compressible benchmark files above 64 KiB, output is
  1.0% to 9.1% smaller, and smaller than Google Brotli q1 on each, at
  0.95x to 1.24x the speed. q1 output changes for these inputs.
- Remove the q1 batch collectors and content heuristics for inputs above
  64 KiB.
- Low-compressibility blocks of 256 KiB at q0 take literal-only meta-blocks
  instead of stored ones, with a code of at most 11 bits from the first 64
  bytes of each KiB, written five literals per store. Blocks whose sample
  saves less than 1 in 32 bits stay stored. Silesia `sao` and `x-ray` go
  from ratio 1.000 to 1.067 and 1.247, at about 1080 MB/s.

## [0.3.3] - 2026-09-25

- Emit token literals in pairs in the q2 to q5 writers, and store length and
  distance extra bits in 32-bit fields. Output is unchanged.
- Keep q5 hash tables in the encoder workspace, so they keep their
  allocation across meta-blocks and reused `Compressor` calls.
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
