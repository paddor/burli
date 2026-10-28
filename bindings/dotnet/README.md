# Burli

Brotli compression and decompression for .NET, built on
[burli](https://github.com/paddor/burli), a Brotli codec in safe Rust. The
API matches the Brotli types of `System.IO.Compression`:

```csharp
using System.IO.Compression;

using (var brotli = new Burli.BrotliStream(output, CompressionLevel.Optimal))
{
    input.CopyTo(brotli);
}
```

`Burli.BrotliStream`, `Burli.BrotliEncoder`, `Burli.BrotliDecoder`, and
`Burli.BrotliCompressionOptions` take the arguments of their
`System.IO.Compression` namesakes and throw the same exceptions. Code can
switch with `using BrotliStream = Burli.BrotliStream;` and the like.

Differences from `System.IO.Compression`:

- Qualities 6 to 11 run as 5, also for `CompressionLevel.SmallestSize`.
- `BrotliStream` throws `InvalidDataException` on a truncated stream, where
  `System.IO.Compression` returns the part it decoded.
- `BrotliDecoder.TryDecompress` returns `false` for data after the end of the
  stream.

The package holds native libraries for Linux (glibc and musl), macOS, and
Windows, on x64 and arm64. .NET 8 or later.
