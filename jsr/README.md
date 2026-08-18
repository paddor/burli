# @paddor/burli

Pure Rust Brotli codec compiled to WebAssembly. The decoder accepts standard
Brotli streams. The encoder supports qualities 0 through 5.

## Usage

```ts
import { compress, decompress, init } from "@paddor/burli";

await init();

const data = new TextEncoder().encode("hello world".repeat(1000));
const compressed = compress(data); // quality 5 by default
const original = decompress(compressed);
```

Select a quality from 0, fastest, through 5, best ratio:

```ts
const compressed = compress(data, { quality: 3 });
```

For untrusted compressed data, set a decompressed-size limit:

```ts
const original = decompress(compressed, {
  maxDecompressedSize: 16 * 1024 * 1024,
});
```

Reusable contexts keep internal work buffers across calls:

```ts
const compressor = new Compressor(4);
const c1 = compressor.compress(data1);
const c2 = compressor.compress(data2);
compressor.free();

const decompressor = new Decompressor({ maxDecompressedSize: 1_000_000 });
const d1 = decompressor.decompress(c1);
const d2 = decompressor.decompress(c2);
decompressor.free();
```

When WASM bytes are already loaded, initialize synchronously:

```ts
import { compress, initSyncFromBytes } from "@paddor/burli";

initSyncFromBytes(wasmBytes);
const compressed = compress(data);
```

## Source

Rust source and native benchmarks:
[github.com/paddor/burli](https://github.com/paddor/burli)
