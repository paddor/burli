# Safety

## Rule

No unsafe in public API. Unsafe, if added, must live in small primitives modules
with a documented contract, `debug_assert!` guards, fuzz coverage, and Miri or
Kani coverage where practical.

## `paranoid`

`cargo test --features paranoid` must compile all workspace crates with
`#![forbid(unsafe_code)]`.

Default build may use unsafe for:

- unaligned little-endian loads;
- fixed-size copy helpers;
- `Vec` initialization after checked writes;
- checked Huffman fast-table indexing.

`paranoid` must swap those for safe equivalents.

## Boundaries

Decoder is highest risk. It handles untrusted bytes. It must:

- never panic on malformed input;
- enforce max output length;
- reject invalid Huffman tables and distances;
- keep ring-buffer copy bounds explicit;
- test truncation, bit flips, splices, and trailing-data policy.

Encoder is lower risk but still must:

- avoid unchecked math overflow;
- keep input indexing guarded;
- keep SIMD scalar-equivalent;
- keep unsupported q6..q11 paths explicit until implemented.

## Upstream bug classes to prevent

- unsafe trait impls;
- allocator failure crashes;
- Rust panics crossing FFI;
- malformed-stream panics;
- move-to-front indexing bugs;
- ring-buffer overflow;
- output-limit bypass;
- base64 mode OOB writes;
- catable stream underflow and invalid concatenation.

## Brotli vulnerabilities to design against

| ID | Bug | burli rule |
|----|-----|------------|
| [CVE-2016-1624](https://nvd.nist.gov/vuln/detail/CVE-2016-1624) | Integer underflow in Chrome's Brotli decoder command execution could trigger buffer overflow or denial of service on crafted input. | Checked distance arithmetic. Bounded ring-buffer copies. Corruption fuzz for command streams. |
| [CVE-2020-8927](https://nvd.nist.gov/vuln/detail/CVE-2020-8927) | Brotli before 1.0.8 had a one-shot decompression buffer overflow when copying chunks larger than 2 GiB. | Bounded-output one-shot API. Streaming-first internals. Checked chunk lengths. |
| [CVE-2020-36846](https://nvd.nist.gov/vuln/detail/CVE-2020-36846) | Binding/package shipped an embedded Brotli copy affected by CVE-2020-8927. | Package smoke tests. Dependency/version audit before release. No hidden bundled codec copies. |
| [CVE-2025-6176](https://nvd.nist.gov/vuln/detail/CVE-2025-6176) | Brotli decompression bomb handling in an HTTP client integration failed to enforce memory limits. | Max output length is first-class. Decode APIs must make limit bypass hard. |
