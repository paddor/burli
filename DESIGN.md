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
when the loop writes them. Block counts, the current command code, and the
distance codes live in locals and change only at block switches.

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
resolve either to direct distances or recent-distance transforms. Invalid zero
distances are rejected before copy execution.

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

- q0: fastest path, sparse low-compressibility sampling, stored-block fallback;
- q1: fast two-pass literal/copy coding with low-compressibility block splitting;
- q2..q4: progressively denser sparse match collection;
- q5: densest implemented scalar path with static dictionary matching.

All q0..q5 outputs use the same standard Brotli format and the same decoder.
Tuning constants live in the encoder tuning module so quality behavior can be
swept without changing match-finder code.

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
