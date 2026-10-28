# burli

Brotli compression and decompression for Ruby, built on
[burli](https://github.com/paddor/burli), a Brotli codec in safe Rust. The
API matches the `brotli` gem:

```ruby
require "burli"

compressed = Burli.deflate(data)
Burli.inflate(compressed) == data # => true
```

Code written for the `brotli` gem can switch with `Brotli = Burli`.
`Burli::Compressor`, `Burli::Decompressor` (with `output_buffer_limit:`),
`Burli::Writer`, and `Burli::Reader` work as in `brotli`. Calls release the
GVL while they encode or decode.

Differences from `brotli`:

- Qualities 6 to 11 run as 5, and the default quality is 5.
- `dictionary:` works for decompression only. Compression with it raises
  `ArgumentError`.
- `Burli.inflate` raises on data after the stream. `Burli::Decompressor`
  keeps such data in `#unused_data`, as in `brotli`.
- Error messages differ, and `Burli.version` returns the gem version.

Gems come precompiled for Linux (glibc and musl), macOS, and Windows. Other
platforms build from source with Rust 1.89 or later.
