# burli

Brotli compression and decompression for Node.js, built on
[burli](https://github.com/paddor/burli), a Brotli codec in safe Rust. The
API matches the Brotli functions of `node:zlib`:

```js
const burli = require("burli");

const compressed = burli.brotliCompressSync(data);
burli.brotliDecompressSync(compressed).equals(data); // true
```

`brotliCompress`, `brotliDecompress`, `createBrotliCompress`,
`createBrotliDecompress`, and `constants` take the options of `node:zlib` and
fail with its error codes. Callback functions and streams run on the libuv
thread pool.

Differences from `node:zlib`:

- Qualities 6 to 11 run as 5, and the default quality is 5.
- Data after the end of the stream is an error. Malformed streams fail with
  code `ERR_BURLI_DECOMPRESSION_FAILED`; truncated ones fail with
  `Z_BUF_ERROR`, as in `node:zlib`.
- `BROTLI_PARAM_SIZE_HINT`, `BROTLI_PARAM_LARGE_WINDOW`,
  `BROTLI_PARAM_NPOSTFIX`, `BROTLI_PARAM_NDIRECT`, and
  `BROTLI_PARAM_DISABLE_LITERAL_CONTEXT_MODELING` have no effect.
- The `info` option is not supported.

Packages come prebuilt for Linux (glibc and musl), macOS, and Windows, on x64
and arm64. Node 22 or later.
