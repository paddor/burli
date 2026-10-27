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
  code up to 4 KiB and two tuned codes up to 16 KiB and 64 KiB. One-shot
  calls on such inputs skip the encoder workspace, which took several KiB
  to clear.
- q0 writes longer inputs in one pass too, in meta-blocks of 256 KiB. Their
  literal codes come from a sample of each block, and their command and
  distance codes from the counts of the block before. This needs a window
  of at least 256 KiB; shorter windows get one-pass meta-blocks of up to
  64 KiB.
- Low-compressibility blocks of 256 KiB at q0 take literal-only
  meta-blocks, with a code of at most 11 bits from the first 64 bytes of
  each KiB. Blocks whose sample saves less than 1 in 32 bits are stored,
  and runs of them go out as one stored meta-block.
- Build the one-pass literal code from the byte values that occur only,
  with an in-place Huffman pass and a table for the zero runs in its
  header.
- q1 compresses inputs of up to 64 KiB in two passes: the matcher collects
  commands with combined insert-and-copy symbols and counts their symbols,
  and each alphabet then gets a Huffman code.
- q1 compresses longer inputs with a matcher like q0's, which pushes its
  commands into the token writer, so they take combined insert-and-copy
  symbols and Huffman codes per meta-block. Text takes matches of 6 bytes
  and more with a table of 2^16 positions, and input with more than 1 in
  12 sampled bytes zero takes 5-byte matches with 2^15.
- Low-compressibility blocks of 64 KiB and more at q1 to q5 get literal
  codes by the top six bits of the byte before, with up to 2, 3, 4, 16,
  and 64 codes merged from 64 context histograms. q1 counts only the first
  128 bytes of each KiB. A meta-block whose literals would take more bytes
  than storing it is stored.
- The q2 to q5 collectors push tokens straight into the writer, which packs
  each into a 24-byte command, instead of collecting 48-byte tokens for the
  whole meta-block first. At q2, an 8 MiB meta-block of Silesia `dickens`
  no longer keeps about 50 MB of tokens.
- Build the q2 to q5 Huffman codes with the q0 and q1 code builder, which
  works only on the symbols that occur and tunes the code-length code.
- Literal-only meta-blocks at q0 and literals by context at q1 to q5 go out
  through one writer, which keeps the bits of the partial byte in a
  register and checks room once per run.
- One-shot calls at every quality return the writer's buffer without a
  copy.
- Remove the q0 collectors and writers, the batch writer that prepared
  every token before writing, and the q1 batch collectors and content
  heuristics for inputs above 64 KiB.
- Against 0.3.5 on the benchmark files above 64 KiB, q0 output is 8.6% to
  27.5% smaller at 0.51x to 1.08x the speed, except `citm-catalog.json`,
  5.8% larger. q1 output is 1.0% to 10.6% smaller at 0.89x to 1.25x the
  speed. On the compressible files, q2 to q5 encode up to 1.67x faster
  with output within 0.1%; Silesia `sao` and `x-ray` at q2 to q5 come out
  4.0% to 16.6% smaller at 0.50x to 0.73x the speed.

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
