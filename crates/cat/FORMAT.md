# Burli Concat Fragment Format v1

Burli concat fragments let callers compress chunks independently and assemble
them into one normal Brotli stream.

This file specifies the serialized on-disk / network representation of one
fragment. It is binary, not ASCII and not JSON.

## RFC 7932 self-contained parts

`assemble_rfc7932_parts` provides a standard Brotli stream mode based on RFC
7932 sections 11.2 and 11.3. Each input part starts a fresh local encoding
state, cannot copy from an earlier part, and ends with an empty metadata
meta-block that aligns the next part to a byte boundary. The final stream
trailer is written only once.

This mode has no Burli-specific fragment header. Use the existing serialized
fragment format when checksums, per-fragment limits, or durable sidecar
metadata are required.

Multi-byte integers are little-endian. All offsets below are byte offsets from
the start of the serialized fragment.

A fragment is not a `.br` file and must not be passed to a normal Brotli
decoder. The assembler reads one or more fragments, writes one Brotli stream
header, appends each fragment payload bitstream in caller order, and writes one
final empty meta-block trailer.

## Serialized Fragment

```text
offset  size  field
0       8     magic = "BURLICAT"
8       1     version_major = 1
9       1     version_minor = 0
10      2     header_len = 72
12      4     flags
16      1     quality
17      1     mode
18      1     lgwin
19      1     block_bits_or_zero
20      1     large_window
21      1     dictionary_policy
22      2     reserved = 0
24      8     input_len
32      8     payload_len
40      8     payload_bit_len
48      1     first_len
49      2     first_bytes
51      1     last_len
52      2     last_bytes
54      2     reserved = 0
56      8     payload_checksum
64      8     header_checksum
72      N     payload bytes
```

`version_major` changes when old decoders must reject the fragment.
`version_minor` changes for backwards-compatible additions. Version 1.0 decoders
must reject any major other than 1. They may accept newer minor versions only if
`header_len = 72`, reserved fields are zero, and all flags are known.

`header_len` lets future minor versions extend the fixed header. Version 1.0
decoders must reject any value other than 72.

`payload_len` is byte count. `payload_bit_len` is count of valid Brotli payload
bits. `ceil(payload_bit_len / 8)` must equal `payload_len`, except both may be
zero for an empty input fragment.

`payload_checksum` is FNV-1a 64-bit over payload bytes.

`header_checksum` is FNV-1a 64-bit over bytes `[0, 64)`, with the checksum field
itself excluded because it starts at offset 64.

Unknown non-zero reserved bytes are invalid in version 1.

## Enum Values

`mode`:

```text
0 = generic
1 = text
2 = font
```

`dictionary_policy`:

```text
0 = disabled
```

`block_bits_or_zero`:

```text
0      = encoder chooses block size
16..24 = explicit Brotli meta-block size log2
```

`large_window`:

```text
0 = standard RFC 7932 window, lgwin 10..24
1 = reserved for future explicit large-window format
```

Version 1 requires `large_window = 0`.

## Flags

```text
bit 0 = no backward references
bit 1 = dictionary disabled
bit 2 = local backward references
bit 3 = prior-state independent
```

Version 1 requires bits 1 and 3 set. Bits other than 0..3 must be clear.
Exactly one payload-kind bit must be set:

```text
bit 0 set, bit 2 clear = payload has no backward copy commands
bit 0 clear, bit 2 set = payload may contain backward copy commands
```

## Stream Layout

```text
Brotli window header
fragment 0 payload bits
fragment 1 payload bits
...
fragment N payload bits
final empty meta-block bits
zero padding in final byte, if needed
```

When `large_window = false`, assembled output is standard RFC 7932 Brotli.
Large-window concat is reserved for a future explicit format version.

## Fragment Payload

Version 1 fragments contain zero or more non-final Brotli meta-blocks:

- no Brotli stream header;
- no final empty meta-block;
- no final compressed meta-block;
- no metadata blocks;
- uncompressed meta-blocks are allowed;
- compressed meta-blocks may contain backward copy commands only when bit 2 is
  set;
- every backward copy distance must be less than or equal to bytes already
  emitted by the same fragment at that copy point;
- no short distance code 0..15;
- no command that reuses the last distance;
- no static dictionary references;
- no raw, shared, or custom dictionary references;
- no literal-context dependency on bytes before the fragment. Version 1
  compressed meta-blocks therefore use exactly one literal tree.

Payload bits are stored in little-endian Brotli bit order. `payload_bit_len`
records the number of valid bits. Unused high bits in the last payload byte must
be zero padding and are covered by the payload checksum.

Version 1 payload validation parses the fragment as Brotli meta-blocks without
a stream header or trailer. It rejects any copy, dictionary, short-distance, or
literal-context state that could depend on bytes before the fragment.

## Sidecar Metadata

The fixed header is the sidecar metadata. It travels with the payload in the
serialized fragment. In memory, APIs may expose it as a `FragmentMetadata`
struct.

API implementations accept an `Options` object with resource limits. Those
limits are not encoded in the fragment. They are local receiver policy.

`first_len` and `last_len` are 0, 1, or 2. Unused bytes in `first_bytes` and
`last_bytes` must be zero.

The assembler validates metadata before writing any output bytes.

## Header And Trailer

Fragments do not contain header bytes or trailer bytes.

The assembler writes the Brotli window header from `ConcatSpec::lgwin`, then
copies fragment payload bits, then writes the Brotli final empty meta-block
bits (`ISLAST=1`, `ISLASTEMPTY=1`). The final byte may contain zero padding.

## Validation Rules

Assembler validation rejects:

- unsupported format version;
- mismatched `ConcatSpec`;
- `large_window = true`;
- configured decoded-size, payload-size, or assembled-size limits;
- invalid `mode`, `dictionary_policy`, `block_bits_or_zero`, or reserved bytes;
- unknown flags;
- header checksum mismatch;
- payload byte length mismatch;
- payload bit length that does not fit payload bytes;
- non-zero payload padding bits;
- checksum mismatch;
- invalid first/last byte lengths;
- missing dictionary-disabled or prior-state-independent flags;
- missing or conflicting payload-kind flags;
- payload-kind flags that disagree with decoded copy commands;
- final or metadata meta-blocks in a fragment;
- short distance codes, distance-ring reuse, dictionary references, or copies
  that reach before the fragment start;
- literal context maps that depend on bytes before the fragment.

Validation happens before mutating caller output.
