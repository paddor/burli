# Design

This document describes implemented internals only.

## Scope

Bürli decodes standard Brotli streams with window sizes 10 through 24. It
rejects large-window streams. The encoder writes standard Brotli streams for
qualities 0 through 5. Qualities 6 through 11 are accepted by option parsing but
return `Unsupported` at encode time.

## API Layers

The public crate exposes three layers:

- one-shot helpers: `compress`, `decompress`, and slice/append variants;
- reusable contexts: `Compressor` and `Decompressor`;
- `std::io` wrappers: `StreamEncoder` and `StreamDecoder`.

Reusable contexts keep internal work buffers between calls. The compressor keeps
match-finder tables, bit-writer capacity, and a scratch buffer for slice output.
The decompressor keeps a scratch output buffer for append and slice calls, so
failed decodes do not partially modify caller buffers.

## Bit I/O

`BitReader` reads Brotli's little-endian bit stream from an immutable byte
slice. It keeps the unread input as a shrinking slice, so a refill tests one
length. Checked methods validate width and remaining input. Trusted methods are
hidden and used only after nearby code has proven enough input bits.

`BitWriter` accumulates pending bits in a 64-bit buffer and flushes full bytes
into an owned `Vec<u8>`. Encoder entry points clear and reuse the same writer in
stateful contexts.

## Decoder

The decoder starts by reading the Brotli window bits, then loops over
meta-block headers:

- `LastEmpty` validates final padding and finishes;
- `Metadata` skips metadata bytes;
- `Uncompressed` byte-aligns and copies literal bytes;
- `Compressed` delegates to the compressed meta-block decoder.

Compressed meta-block decoding reads block category metadata, literal,
command, and distance prefix codes, then executes the command stream. Output
limit checks happen before each meta-block body can grow the output.

The command loop writes into zeroed room past the decoded bytes, as a slice.
The output grows 64 KiB at a time, up to the meta-block end plus 32 bytes of
slack, so writes need no capacity checks and the zeroed lines are still cached
when the loop writes them. Block counts, the current command code, the
distance codes, and the literal context map and mode live in locals and
change only at block switches.

## Huffman Tables

Prefix codes are canonical Huffman codes. The codes of one alphabet in a
meta-block share one buffer: a 256-entry root per code, then second-level
tables for codes longer than 8 bits. An entry packs a symbol and its code
length. A root entry for longer codes packs the offset of their second-level
table and its total width. A single-symbol code fills its root with
zero-length entries, so it decodes like any other code.

Reading code lengths links the symbols of each length into a list in symbol
order. That is canonical order, so building a table walks the lists, with no
length array and no sort. The root fills to the width of its longest code and
then doubles. Second-level sizes come from the length counts first, so the
buffer grows once per code.

A meta-block of 4 KiB or more with one literal code also builds a 10-bit root
when codes longer than 8 bits cover over 1/8 of the code space. Binary data
has many 9- to 11-bit literal codes, and each second-level lookup costs a
mispredicted branch. Literal context maps that pick the same tree for all 64
contexts decode like a single code.

The hot decode path reads the root entry for the low buffered bits, follows a
second-level entry for long codes, drops the code length, and returns the
symbol. Roots are fixed-size arrays indexed by masked bits, so lookups need no
bounds checks. Bits past the input end read as zero, and the decoder checks
the code length against the bits left.

## Backward Copies

The decoder tracks Brotli's four-entry recent-distance ring. Distance symbols
resolve either to direct distances or recent-distance transforms. One test
sends a distance within both the output and the window to the copy. Every
other distance takes a cold path, which resolves dictionary references and
rejects the rest, zero included.

Copy execution:

- a copy of up to 32 bytes that does not overlap its source moves one whole
  16- or 32-byte window; bytes past the copy land in room that later writes
  replace;
- longer non-overlapping copies use `copy_within`;
- overlapping copies repeat the pattern in 16- or 8-byte chunks; distance 1
  fills, and distances 2 to 7 first write the pattern up to 8 bytes or more.

## Dictionaries

Dictionary references resolve through the shared Brotli dictionary table and
transform code. The transform path validates length classes and UTF-8-sensitive
uppercase transforms before appending bytes.

A table built at compile time holds each transform's prefix and suffix
inline. With room past the word, the prefix, word, and suffix move as fixed
8-, 24-, and 8-byte windows. Each window runs into bytes that the next part
or later output replaces, as short backward copies do.

Raw LZ77 prefix dictionaries are supported on decode. They are kept separate
from the output ring. Distances in the range after the normal backward window
copy from the raw dictionary suffix first, then continue into the beginning of
the regular LZ77 window if the copy length crosses the dictionary end. Static
dictionary distance IDs are shifted by the raw dictionary length, matching the
shared Brotli dictionary rules.

Serialized shared dictionaries and custom static word/transform dictionaries
are not implemented.

## Encoder

The encoder writes one or more meta-blocks followed by the final empty
meta-block. If a compressed plan is not smaller than stored output, the encoder
falls back to a stored stream.

Quality policy is intentionally narrow:

- q0: fastest path, one pass, sampled literal codes, literal-only or stored
  blocks for low-compressibility input;
- q1: two passes, with Huffman codes from the counts of the commands;
- q2..q4: progressively denser sparse match collection;
- q5: densest implemented scalar path with static dictionary matching.

All q0..q5 outputs use the same standard Brotli format and the same decoder.
Tuning constants live in the encoder tuning module so quality behavior can be
swept without changing match-finder code.

### q0 One-Pass Encoder

q0 writes meta-blocks of up to 64 KiB in one pass, like Google's fast
fragment encoder. It writes each match as soon as it finds it, so there is
no command buffer and no second pass:

- The literal code comes from the byte counts of the whole input, taken
  before matching. The code builder works only on the byte values that
  occur.
- Commands and distances use fixed codes whose headers are stored as bits:
  a flat code up to 4 KiB, and two codes tuned on Silesia and web data for
  inputs up to 16 KiB and 64 KiB. Each takes Brotli's combined
  insert-and-copy symbols.
- The hash table holds `u16` positions and grows with the input, with an
  odd number of index bits as in Google's encoder.
- Output goes through a sink with a bound on its size. When the meta-block
  would come out longer than the input, the encoder rewinds and stores it.
- Inputs that the low-compressibility sampler rejects are stored without
  matching.

One-shot q0 calls on inputs of up to 64 KiB skip the encoder workspace.

Longer inputs take one-pass meta-blocks of 256 KiB, which copy from earlier
ones:

- The literal code comes from a sample of every 29th byte of the block.
- Command and distance codes come from the counts of the block before,
  with every symbol of a fixed long-input code counted once more so that
  each has a code. The first block, and the first after stored ones, start
  from counts made from that fixed code.
- The hash table holds 2^15 `u32` entries that keep the low 18 bits of a
  position, so copies reach back up to 256 KiB with no distance check.
- Blocks that look incompressible take a literal-only meta-block. Its
  code comes from the first 64 bytes of each KiB and has codes of at most
  11 bits, so five literals go out in one 64-bit store. Blocks whose
  sample saves less than 1 in 32 bits by its entropy are stored, and runs
  of them go out as one stored meta-block.

This needs a window of at least 256 KiB. With a shorter one, the one-pass
encoder writes meta-blocks of up to 64 KiB, one per window.

### q1 Two-Pass Encoder

q1 inputs of up to 64 KiB take two passes. The q1 matcher collects commands
with combined insert-and-copy symbols and counts literals, commands, and
distances as it goes. Each alphabet then gets a Huffman code from its
counts, and the commands go out in one pass.

Longer inputs take a matcher like q0's, with whole `u32` positions and
copies up to 256 KiB back. It pushes each command into the q2 to q5 token
writer below, which ends meta-blocks by symbol count. Text takes matches of
6 bytes and more, with a table of 2^16 positions. Input with more than 1 in
12 of 4096 sampled bytes zero takes 5-byte matches and 2^15 positions, as
binary code and data repeat shorter strings. Larger tables or more
positions per copy would compress text past q2.

### q2 to q5 Token Writer

The q2 to q5 collectors push each token into a writer as they find it, so
no list of all tokens is kept. The writer packs a token into a 24-byte
command with its symbols and extra bits and counts its literals, command,
and distance. Once a meta-block holds a symbol limit of literals and
commands, each alphabet gets a Huffman code from its counts, and the
commands go out in one pass through a bit sink sized from those counts.
q2 meta-blocks of up to 1024 commands use fixed command and distance
codes instead.

### Low-Compressibility Literals

Blocks that the low-compressibility sampler flags take literal-only
meta-blocks. From 64 KiB up at q2 to q5, literals are coded by the top six
bits of the byte before, as binary data such as 16-bit samples often depends
on it. The 64 context histograms merge greedily into at most 2, 4, 16, or
64 codes by quality, always the pair whose merge costs the fewest estimated
bits: first among neighboring contexts, then at q4 and q5 among all pairs.

## Entropy Coding

Literal, command, and distance streams are converted to Brotli prefix codes.
Small symbol sets use simple prefix-code encoding when possible. Larger sets use
canonical code lengths built from observed frequencies and then serialize the
code-length tree.

## Streaming

`StreamEncoder` buffers input until the configured block size and writes each
chunk as a meta-block. It keeps the encoder workspace and bit writer across
writes. `finish` flushes buffered input, writes the final empty meta-block, and
returns the wrapped writer.

`StreamDecoder` reads encoded bytes in 8 KiB chunks. It keeps encoded bytes that
may still contain unread bits and keeps only the decoded window history needed
for future backward copies. Reads return decoded bytes as soon as a meta-block
produces output.

## Safety

No crate has `unsafe`; every crate is `#![forbid(unsafe_code)]`. The
`paranoid` feature has no effect today. If a later release adds `unsafe` for
speed, builds with `paranoid` leave it out.
