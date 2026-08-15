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
slice. Checked methods validate width and remaining input. Trusted methods are
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

## Huffman Tables

Prefix codes are canonical Huffman codes. Each non-single code builds a fixed
fast lookup table with `1 << fast_bits` entries. A fast entry stores symbol and
bit length in a packed value. Single-symbol codes bypass bit reads.

The hot decode path peeks bits, reads the fast lookup entry, drops the encoded
bit length, and returns the symbol. The padded lookup path is cold and handles
valid streams that end with fewer physical bits than the table width.

## Backward Copies

The decoder tracks Brotli's four-entry recent-distance ring. Distance symbols
resolve either to direct distances or recent-distance transforms. Invalid zero
distances are rejected before copy execution.

Copy execution has three paths:

- distance 1 repeats the previous byte;
- non-overlapping copies use one bulk copy in the default build;
- overlapping copies extend from already-produced bytes in chunks.

The default build uses a small unsafe copy primitive for non-overlapping
backward copies. The `paranoid` feature replaces it with safe
`Vec::extend_from_within`.

## Static Dictionary

Dictionary references resolve through the shared Brotli dictionary table and
transform code. The transform path validates length classes and UTF-8-sensitive
uppercase transforms before appending bytes.

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

## SIMD

SIMD dispatch is wired through features and shared primitive modules. Current
hot paths are still mostly scalar. `paranoid` keeps the same feature surface and
must continue to build with `forbid(unsafe_code)`.

## Unsafe Boundary

Unsafe code is allowed only in small primitives with local contracts and
`debug_assert!` guards. Current default-build unsafe is limited to:

- unaligned little-endian loads in the encoder match scanner;
- non-overlapping backward-copy append in the decoder;
- trusted literal bulk writes in the decoder;
- checked Huffman fast-table indexing.

The `paranoid` feature swaps these paths for safe equivalents.
