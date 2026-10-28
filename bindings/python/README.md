# burli

Brotli compression and decompression for Python, built on
[burli](https://github.com/paddor/burli), a Brotli codec in safe Rust. The
API matches Google's `brotli` module:

```python
import burli as brotli

compressed = brotli.compress(data)
assert brotli.decompress(compressed) == data
```

`Compressor` and `Decompressor` work as in `brotli`, including
`Decompressor.process(data, output_buffer_limit=...)` and
`can_accept_more_data()`, which urllib3 uses to bound decompressed output.
Calls release the GIL while they encode or decode.

Differences from `brotli`:

- Qualities 6 to 11 run as 5, and the default quality is 5.
- With `output_buffer_limit`, `process` returns at most that many bytes, and
  `can_accept_more_data()` also returns `False` while the decoder holds
  output.
- Error messages differ. Errors raise `burli.error`.
